use std::collections::VecDeque;
use std::future::Future;
use std::io::Cursor;
use std::os::unix::prelude::MetadataExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::task::{Poll, Waker};
use std::time::Duration;

use bytes::{Buf, BytesMut};
use futures::future::{BoxFuture, Fuse};
use futures::{AsyncSeek, FutureExt, TryStreamExt};
use helpers::NonDetachingJoinHandle;
use nix::unistd::{Gid, Uid};
use tokio::fs::File;
use tokio::io::{
    duplex, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf, WriteHalf,
};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::time::{Instant, Sleep};

use crate::prelude::*;

pub trait AsyncReadSeek: AsyncRead + AsyncSeek {}
impl<T: AsyncRead + AsyncSeek> AsyncReadSeek for T {}

#[derive(Clone, Debug)]
pub struct AsyncCompat<T>(pub T);
impl<T> futures::io::AsyncRead for AsyncCompat<T>
where
    T: tokio::io::AsyncRead,
{
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let mut read_buf = ReadBuf::new(buf);
        tokio::io::AsyncRead::poll_read(
            unsafe { self.map_unchecked_mut(|a| &mut a.0) },
            cx,
            &mut read_buf,
        )
        .map(|res| res.map(|_| read_buf.filled().len()))
    }
}
impl<T> tokio::io::AsyncRead for AsyncCompat<T>
where
    T: futures::io::AsyncRead,
{
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf,
    ) -> std::task::Poll<std::io::Result<()>> {
        futures::io::AsyncRead::poll_read(
            unsafe { self.map_unchecked_mut(|a| &mut a.0) },
            cx,
            buf.initialize_unfilled(),
        )
        .map(|res| res.map(|len| buf.set_filled(len)))
    }
}
impl<T> futures::io::AsyncWrite for AsyncCompat<T>
where
    T: tokio::io::AsyncWrite,
{
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        tokio::io::AsyncWrite::poll_write(unsafe { self.map_unchecked_mut(|a| &mut a.0) }, cx, buf)
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        tokio::io::AsyncWrite::poll_flush(unsafe { self.map_unchecked_mut(|a| &mut a.0) }, cx)
    }
    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        tokio::io::AsyncWrite::poll_shutdown(unsafe { self.map_unchecked_mut(|a| &mut a.0) }, cx)
    }
}
impl<T> tokio::io::AsyncWrite for AsyncCompat<T>
where
    T: futures::io::AsyncWrite,
{
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        futures::io::AsyncWrite::poll_write(
            unsafe { self.map_unchecked_mut(|a| &mut a.0) },
            cx,
            buf,
        )
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        futures::io::AsyncWrite::poll_flush(unsafe { self.map_unchecked_mut(|a| &mut a.0) }, cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        futures::io::AsyncWrite::poll_close(unsafe { self.map_unchecked_mut(|a| &mut a.0) }, cx)
    }
}

pub async fn from_yaml_async_reader<T, R>(mut reader: R) -> Result<T, crate::Error>
where
    T: for<'de> serde::Deserialize<'de>,
    R: AsyncRead + Unpin,
{
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;
    serde_yaml::from_slice(&buffer)
        .map_err(color_eyre::eyre::Error::from)
        .with_kind(crate::ErrorKind::Deserialization)
}

pub async fn to_yaml_async_writer<T, W>(mut writer: W, value: &T) -> Result<(), crate::Error>
where
    T: serde::Serialize,
    W: AsyncWrite + Unpin,
{
    let mut buffer = serde_yaml::to_string(value)
        .with_kind(crate::ErrorKind::Serialization)?
        .into_bytes();
    buffer.extend_from_slice(b"\n");
    writer.write_all(&buffer).await?;
    Ok(())
}

pub async fn from_toml_async_reader<T, R>(mut reader: R) -> Result<T, crate::Error>
where
    T: for<'de> serde::Deserialize<'de>,
    R: AsyncRead + Unpin,
{
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;
    serde_toml::from_str(std::str::from_utf8(&buffer)?)
        .map_err(color_eyre::eyre::Error::from)
        .with_kind(crate::ErrorKind::Deserialization)
}

pub async fn to_toml_async_writer<T, W>(mut writer: W, value: &T) -> Result<(), crate::Error>
where
    T: serde::Serialize,
    W: AsyncWrite + Unpin,
{
    let mut buffer = serde_toml::to_string(value)
        .with_kind(crate::ErrorKind::Serialization)?
        .into_bytes();
    buffer.extend_from_slice(b"\n");
    writer.write_all(&buffer).await?;
    Ok(())
}

pub async fn from_cbor_async_reader<T, R>(mut reader: R) -> Result<T, crate::Error>
where
    T: for<'de> serde::Deserialize<'de>,
    R: AsyncRead + Unpin,
{
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;
    serde_cbor::de::from_reader(buffer.as_slice())
        .map_err(color_eyre::eyre::Error::from)
        .with_kind(crate::ErrorKind::Deserialization)
}
pub async fn to_cbor_async_writer<T, W>(mut writer: W, value: &T) -> Result<(), crate::Error>
where
    T: serde::Serialize,
    W: AsyncWrite + Unpin,
{
    let mut buffer = Vec::new();
    serde_cbor::ser::into_writer(value, &mut buffer).with_kind(crate::ErrorKind::Serialization)?;
    buffer.extend_from_slice(b"\n");
    writer.write_all(&buffer).await?;
    Ok(())
}

pub async fn from_json_async_reader<T, R>(mut reader: R) -> Result<T, crate::Error>
where
    T: for<'de> serde::Deserialize<'de>,
    R: AsyncRead + Unpin,
{
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;
    serde_json::from_slice(&buffer)
        .map_err(color_eyre::eyre::Error::from)
        .with_kind(crate::ErrorKind::Deserialization)
}

pub async fn to_json_async_writer<T, W>(mut writer: W, value: &T) -> Result<(), crate::Error>
where
    T: serde::Serialize,
    W: AsyncWrite + Unpin,
{
    let buffer = serde_json::to_string(value).with_kind(crate::ErrorKind::Serialization)?;
    writer.write_all(&buffer.as_bytes()).await?;
    Ok(())
}

pub async fn to_json_pretty_async_writer<T, W>(mut writer: W, value: &T) -> Result<(), crate::Error>
where
    T: serde::Serialize,
    W: AsyncWrite + Unpin,
{
    let mut buffer =
        serde_json::to_string_pretty(value).with_kind(crate::ErrorKind::Serialization)?;
    buffer.push_str("\n");
    writer.write_all(&buffer.as_bytes()).await?;
    Ok(())
}

pub async fn copy_and_shutdown<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    r: &mut R,
    mut w: W,
) -> Result<(), std::io::Error> {
    tokio::io::copy(r, &mut w).await?;
    w.flush().await?;
    w.shutdown().await?;
    Ok(())
}

pub fn dir_size<'a, P: AsRef<Path> + 'a + Send + Sync>(
    path: P,
    ctr: Option<&'a Counter>,
) -> BoxFuture<'a, Result<u64, std::io::Error>> {
    async move {
        tokio_stream::wrappers::ReadDirStream::new(tokio::fs::read_dir(path.as_ref()).await?)
            .try_fold(0, |acc, e| async move {
                let m = e.metadata().await?;
                Ok(acc
                    + if m.is_file() {
                        if let Some(ctr) = ctr {
                            ctr.add(m.len());
                        }
                        m.len()
                    } else if m.is_dir() {
                        dir_size(e.path(), ctr).await?
                    } else {
                        0
                    })
            })
            .await
    }
    .boxed()
}

pub fn response_to_reader(response: reqwest::Response) -> impl AsyncRead + Unpin {
    tokio_util::io::StreamReader::new(response.bytes_stream().map_err(|e| {
        std::io::Error::new(
            if e.is_connect() {
                std::io::ErrorKind::ConnectionRefused
            } else if e.is_timeout() {
                std::io::ErrorKind::TimedOut
            } else {
                std::io::ErrorKind::Other
            },
            e,
        )
    }))
}

#[pin_project::pin_project]
pub struct BufferedWriteReader {
    #[pin]
    hdl: Fuse<NonDetachingJoinHandle<Result<(), std::io::Error>>>,
    #[pin]
    rdr: DuplexStream,
}
impl BufferedWriteReader {
    pub fn new<
        W: FnOnce(WriteHalf<DuplexStream>) -> Fut,
        Fut: Future<Output = Result<(), std::io::Error>> + Send + Sync + 'static,
    >(
        write_fn: W,
        max_buf_size: usize,
    ) -> Self {
        let (w, rdr) = duplex(max_buf_size);
        let (_, w) = tokio::io::split(w);
        BufferedWriteReader {
            hdl: NonDetachingJoinHandle::from(tokio::spawn(write_fn(w))).fuse(),
            rdr,
        }
    }
}
impl AsyncRead for BufferedWriteReader {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.project();
        let res = this.rdr.poll_read(cx, buf);
        match this.hdl.poll(cx) {
            Poll::Ready(Ok(Err(e))) => return Poll::Ready(Err(e)),
            Poll::Ready(Err(e)) => {
                return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, e)))
            }
            _ => res,
        }
    }
}

pub trait CursorExt {
    fn pure_read(&mut self, buf: &mut ReadBuf<'_>);
    fn remaining_slice(&self) -> &[u8];
}

impl<T: AsRef<[u8]>> CursorExt for Cursor<T> {
    fn pure_read(&mut self, buf: &mut ReadBuf<'_>) {
        let end = self.position() as usize
            + std::cmp::min(
                buf.remaining(),
                self.get_ref().as_ref().len() - self.position() as usize,
            );
        buf.put_slice(&self.get_ref().as_ref()[self.position() as usize..end]);
        self.set_position(end as u64);
    }
    fn remaining_slice(&self) -> &[u8] {
        let len = self.position().min(self.get_ref().as_ref().len() as u64);
        &self.get_ref().as_ref()[(len as usize)..]
    }
}

#[pin_project::pin_project]
#[derive(Debug)]
pub struct BackTrackingReader<T> {
    #[pin]
    reader: T,
    buffer: Cursor<Vec<u8>>,
    buffering: bool,
}
impl<T> BackTrackingReader<T> {
    pub fn new(reader: T) -> Self {
        Self {
            reader,
            buffer: Cursor::new(Vec::new()),
            buffering: false,
        }
    }
    pub fn start_buffering(&mut self) {
        self.buffer.set_position(0);
        self.buffer.get_mut().truncate(0);
        self.buffering = true;
    }
    pub fn stop_buffering(&mut self) {
        self.buffer.set_position(0);
        self.buffer.get_mut().truncate(0);
        self.buffering = false;
    }
    pub fn rewind(&mut self) {
        self.buffering = false;
    }
    pub fn unwrap(self) -> T {
        self.reader
    }
}

impl<T: AsyncRead> AsyncRead for BackTrackingReader<T> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.project();
        if *this.buffering {
            let filled = buf.filled().len();
            let res = this.reader.poll_read(cx, buf);
            this.buffer
                .get_mut()
                .extend_from_slice(&buf.filled()[filled..]);
            res
        } else {
            let mut ready = false;
            if (this.buffer.position() as usize) < this.buffer.get_ref().len() {
                this.buffer.pure_read(buf);
                ready = true;
            }
            if buf.remaining() > 0 {
                match this.reader.poll_read(cx, buf) {
                    Poll::Pending => {
                        if ready {
                            Poll::Ready(Ok(()))
                        } else {
                            Poll::Pending
                        }
                    }
                    a => a,
                }
            } else {
                Poll::Ready(Ok(()))
            }
        }
    }
}

impl<T: AsyncWrite> AsyncWrite for BackTrackingReader<T> {
    fn is_write_vectored(&self) -> bool {
        self.reader.is_write_vectored()
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        self.project().reader.poll_flush(cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        self.project().reader.poll_shutdown(cx)
    }
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        self.project().reader.poll_write(cx, buf)
    }
    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        self.project().reader.poll_write_vectored(cx, bufs)
    }
}

pub struct Counter {
    atomic: AtomicU64,
    ordering: std::sync::atomic::Ordering,
}
impl Counter {
    pub fn new(init: u64, ordering: std::sync::atomic::Ordering) -> Self {
        Self {
            atomic: AtomicU64::new(init),
            ordering,
        }
    }
    pub fn load(&self) -> u64 {
        self.atomic.load(self.ordering)
    }
    pub fn add(&self, value: u64) {
        self.atomic.fetch_add(value, self.ordering);
    }
}

#[pin_project::pin_project]
pub struct CountingReader<'a, R> {
    ctr: &'a Counter,
    #[pin]
    rdr: R,
}
impl<'a, R> CountingReader<'a, R> {
    pub fn new(rdr: R, ctr: &'a Counter) -> Self {
        Self { ctr, rdr }
    }
    pub fn into_inner(self) -> R {
        self.rdr
    }
}
impl<'a, R: AsyncRead> AsyncRead for CountingReader<'a, R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.project();
        let start = buf.filled().len();
        let res = this.rdr.poll_read(cx, buf);
        let len = buf.filled().len() - start;
        if len > 0 {
            this.ctr.add(len as u64);
        }
        res
    }
}

pub fn dir_copy<'a, P0: AsRef<Path> + 'a + Send + Sync, P1: AsRef<Path> + 'a + Send + Sync>(
    src: P0,
    dst: P1,
    ctr: Option<&'a Counter>,
) -> BoxFuture<'a, Result<(), crate::Error>> {
    async move {
        let m = tokio::fs::metadata(&src).await?;
        let dst_path = dst.as_ref();
        tokio::fs::create_dir_all(&dst_path).await.with_ctx(|_| {
            (
                crate::ErrorKind::Filesystem,
                format!("mkdir {}", dst_path.display()),
            )
        })?;
        tokio::fs::set_permissions(&dst_path, m.permissions())
            .await
            .with_ctx(|_| {
                (
                    crate::ErrorKind::Filesystem,
                    format!("chmod {}", dst_path.display()),
                )
            })?;
        let tmp_dst_path = dst_path.to_owned();
        tokio::task::spawn_blocking(move || {
            nix::unistd::chown(
                &tmp_dst_path,
                Some(Uid::from_raw(m.uid())),
                Some(Gid::from_raw(m.gid())),
            )
        })
        .await
        .with_kind(crate::ErrorKind::Unknown)?
        .with_ctx(|_| {
            (
                crate::ErrorKind::Filesystem,
                format!("chown {}", dst_path.display()),
            )
        })?;
        tokio_stream::wrappers::ReadDirStream::new(tokio::fs::read_dir(src.as_ref()).await?)
            .map_err(|e| crate::Error::new(e, crate::ErrorKind::Filesystem))
            .try_for_each(|e| async move {
                let m = e.metadata().await?;
                let src_path = e.path();
                let dst_path = dst_path.join(e.file_name());
                if m.is_file() {
                    let mut dst_file = tokio::fs::File::create(&dst_path).await.with_ctx(|_| {
                        (
                            crate::ErrorKind::Filesystem,
                            format!("create {}", dst_path.display()),
                        )
                    })?;
                    let mut rdr = tokio::fs::File::open(&src_path).await.with_ctx(|_| {
                        (
                            crate::ErrorKind::Filesystem,
                            format!("open {}", src_path.display()),
                        )
                    })?;
                    if let Some(ctr) = ctr {
                        tokio::io::copy(&mut CountingReader::new(rdr, ctr), &mut dst_file).await
                    } else {
                        tokio::io::copy(&mut rdr, &mut dst_file).await
                    }
                    .with_ctx(|_| {
                        (
                            crate::ErrorKind::Filesystem,
                            format!("cp {} -> {}", src_path.display(), dst_path.display()),
                        )
                    })?;
                    dst_file.flush().await?;
                    dst_file.shutdown().await?;
                    dst_file.sync_all().await?;
                    drop(dst_file);
                    let tmp_dst_path = dst_path.clone();
                    tokio::task::spawn_blocking(move || {
                        nix::unistd::chown(
                            &tmp_dst_path,
                            Some(Uid::from_raw(m.uid())),
                            Some(Gid::from_raw(m.gid())),
                        )
                    })
                    .await
                    .with_kind(crate::ErrorKind::Unknown)?
                    .with_ctx(|_| {
                        (
                            crate::ErrorKind::Filesystem,
                            format!("chown {}", dst_path.display()),
                        )
                    })?;
                } else if m.is_dir() {
                    dir_copy(src_path, dst_path, ctr).await?;
                } else if m.file_type().is_symlink() {
                    tokio::fs::symlink(
                        tokio::fs::read_link(&src_path).await.with_ctx(|_| {
                            (
                                crate::ErrorKind::Filesystem,
                                format!("readlink {}", src_path.display()),
                            )
                        })?,
                        &dst_path,
                    )
                    .await
                    .with_ctx(|_| {
                        (
                            crate::ErrorKind::Filesystem,
                            format!("cp -P {} -> {}", src_path.display(), dst_path.display()),
                        )
                    })?;
                    // Do not set permissions (see https://unix.stackexchange.com/questions/87200/change-permissions-for-a-symbolic-link)
                }
                Ok(())
            })
            .await?;
        Ok(())
    }
    .boxed()
}

#[pin_project::pin_project]
pub struct TimeoutStream<S: AsyncRead + AsyncWrite = TcpStream> {
    timeout: Duration,
    #[pin]
    sleep: Sleep,
    #[pin]
    stream: S,
}
impl<S: AsyncRead + AsyncWrite> TimeoutStream<S> {
    pub fn new(stream: S, timeout: Duration) -> Self {
        Self {
            timeout,
            sleep: tokio::time::sleep(timeout),
            stream,
        }
    }
}
impl<S: AsyncRead + AsyncWrite> AsyncRead for TimeoutStream<S> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let mut this = self.project();
        if let std::task::Poll::Ready(_) = this.sleep.as_mut().poll(cx) {
            return std::task::Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out",
            )));
        }
        let res = this.stream.poll_read(cx, buf);
        if res.is_ready() {
            this.sleep.reset(Instant::now() + *this.timeout);
        }
        res
    }
}
impl<S: AsyncRead + AsyncWrite> AsyncWrite for TimeoutStream<S> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        let this = self.project();
        let res = this.stream.poll_write(cx, buf);
        if res.is_ready() {
            this.sleep.reset(Instant::now() + *this.timeout);
        }
        res
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let this = self.project();
        let res = this.stream.poll_flush(cx);
        if res.is_ready() {
            this.sleep.reset(Instant::now() + *this.timeout);
        }
        res
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let this = self.project();
        let res = this.stream.poll_shutdown(cx);
        if res.is_ready() {
            this.sleep.reset(Instant::now() + *this.timeout);
        }
        res
    }
}

pub struct TmpFile {}

#[derive(Debug)]
pub struct TmpDir {
    path: PathBuf,
}
impl TmpDir {
    pub async fn new() -> Result<Self, Error> {
        let path = Path::new("/var/tmp/startos").join(base32::encode(
            base32::Alphabet::RFC4648 { padding: false },
            &rand::random::<[u8; 8]>(),
        ));
        if tokio::fs::metadata(&path).await.is_ok() {
            return Err(Error::new(
                eyre!("{path:?} already exists"),
                ErrorKind::Filesystem,
            ));
        }
        tokio::fs::create_dir_all(&path).await?;
        Ok(Self { path })
    }

    pub async fn delete(self) -> Result<(), Error> {
        tokio::fs::remove_dir_all(&self.path).await?;
        Ok(())
    }
}
impl std::ops::Deref for TmpDir {
    type Target = Path;
    fn deref(&self) -> &Self::Target {
        &self.path
    }
}
impl AsRef<Path> for TmpDir {
    fn as_ref(&self) -> &Path {
        &*self
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        if self.path.exists() {
            let path = std::mem::take(&mut self.path);
            tokio::spawn(async move {
                tokio::fs::remove_dir_all(&path).await.unwrap();
            });
        }
    }
}

pub async fn create_file(path: impl AsRef<Path>) -> Result<File, Error> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_ctx(|_| (ErrorKind::Filesystem, lazy_format!("mkdir -p {parent:?}")))?;
    }
    File::create(path)
        .await
        .with_ctx(|_| (ErrorKind::Filesystem, lazy_format!("create {path:?}")))
}

pub async fn rename(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    let src = src.as_ref();
    let dst = dst.as_ref();
    if let Some(parent) = dst.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_ctx(|_| (ErrorKind::Filesystem, lazy_format!("mkdir -p {parent:?}")))?;
    }
    tokio::fs::rename(src, dst)
        .await
        .with_ctx(|_| (ErrorKind::Filesystem, lazy_format!("mv {src:?} -> {dst:?}")))
}

fn poll_flush_prefix<W: AsyncWrite>(
    mut writer: Pin<&mut W>,
    cx: &mut std::task::Context<'_>,
    prefix: &mut VecDeque<Cursor<Vec<u8>>>,
    flush_writer: bool,
) -> Poll<Result<(), std::io::Error>> {
    while let Some(mut cur) = prefix.pop_front() {
        let buf = cur.remaining_slice();
        if !buf.is_empty() {
            match writer.as_mut().poll_write(cx, buf)? {
                Poll::Ready(n) if n == buf.len() => (),
                Poll::Ready(n) => {
                    cur.advance(n);
                    prefix.push_front(cur);
                }
                Poll::Pending => {
                    prefix.push_front(cur);
                    return Poll::Pending;
                }
            }
        }
    }
    if flush_writer {
        writer.poll_flush(cx)
    } else {
        Poll::Ready(Ok(()))
    }
}

fn poll_write_prefix_buf<W: AsyncWrite>(
    mut writer: Pin<&mut W>,
    cx: &mut std::task::Context<'_>,
    prefix: &mut VecDeque<Cursor<Vec<u8>>>,
    buf: &[u8],
) -> Poll<Result<usize, std::io::Error>> {
    futures::ready!(poll_flush_prefix(writer.as_mut(), cx, prefix, false)?);
    writer.poll_write(cx, buf)
}

#[pin_project::pin_project]
pub struct TeeWriter<W1, W2> {
    capacity: usize,
    buffer1: VecDeque<Cursor<Vec<u8>>>,
    buffer2: VecDeque<Cursor<Vec<u8>>>,
    #[pin]
    writer1: W1,
    #[pin]
    writer2: W2,
}
impl<W1: AsyncWrite, W2: AsyncWrite> TeeWriter<W1, W2> {
    pub fn new(writer1: W1, writer2: W2, capacity: usize) -> Self {
        Self {
            capacity,
            buffer1: VecDeque::new(),
            buffer2: VecDeque::new(),
            writer1,
            writer2,
        }
    }
}

impl<W1: AsyncWrite + Unpin, W2: AsyncWrite + Unpin> TeeWriter<W1, W2> {
    pub async fn into_inner(mut self) -> Result<(W1, W2), Error> {
        self.flush().await?;

        Ok((self.writer1, self.writer2))
    }
}
impl<W1: AsyncWrite, W2: AsyncWrite> AsyncWrite for TeeWriter<W1, W2> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        mut buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let mut this = self.project();
        let buffer_size = this
            .buffer1
            .iter()
            .chain(this.buffer2.iter())
            .map(|b| b.get_ref().len())
            .sum::<usize>();
        if buffer_size < *this.capacity {
            let to_write = std::cmp::min(*this.capacity - buffer_size, buf.len());
            buf = &buf[0..to_write];
        } else {
            match (
                poll_flush_prefix(this.writer1.as_mut(), cx, &mut this.buffer1, false)?,
                poll_flush_prefix(this.writer2.as_mut(), cx, &mut this.buffer2, false)?,
            ) {
                (Poll::Ready(()), Poll::Ready(())) => (),
                _ => return Poll::Pending,
            }
        }
        let (w1, w2) = match (
            poll_write_prefix_buf(this.writer1.as_mut(), cx, &mut this.buffer1, buf)?,
            poll_write_prefix_buf(this.writer2.as_mut(), cx, &mut this.buffer2, buf)?,
        ) {
            (Poll::Pending, Poll::Pending) => return Poll::Pending,
            (Poll::Ready(n), Poll::Pending) => (n, 0),
            (Poll::Pending, Poll::Ready(n)) => (0, n),
            (Poll::Ready(n1), Poll::Ready(n2)) => (n1, n2),
        };
        if w1 > w2 {
            this.buffer2.push_back(Cursor::new(buf[w2..w1].to_vec()));
        } else if w1 < w2 {
            this.buffer1.push_back(Cursor::new(buf[w1..w2].to_vec()));
        }
        Poll::Ready(Ok(std::cmp::max(w1, w2)))
    }
    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let mut this = self.project();
        match (
            poll_flush_prefix(this.writer1, cx, &mut this.buffer1, true)?,
            poll_flush_prefix(this.writer2, cx, &mut this.buffer2, true)?,
        ) {
            (Poll::Ready(()), Poll::Ready(())) => Poll::Ready(Ok(())),
            _ => Poll::Pending,
        }
    }
    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        self.poll_flush(cx)
    }
}

#[pin_project::pin_project]
pub struct ParallelBlake3Writer {
    #[pin]
    hasher: NonDetachingJoinHandle<blake3::Hash>,
    buffer: Arc<(std::sync::Mutex<(BytesMut, Vec<Waker>, bool)>, Notify)>,
    capacity: usize,
}
impl ParallelBlake3Writer {
    /// memory usage can be as much as 2x capacity
    pub fn new(capacity: usize) -> Self {
        let buffer = Arc::new((
            std::sync::Mutex::new((BytesMut::new(), Vec::<Waker>::new(), false)),
            Notify::new(),
        ));
        let hasher_buffer = buffer.clone();
        Self {
            hasher: tokio::spawn(async move {
                let mut hasher = blake3::Hasher::new();
                let mut to_hash = BytesMut::new();
                let mut notified;
                while {
                    let mut guard = hasher_buffer.0.lock().unwrap();
                    let (buffer, wakers, shutdown) = &mut *guard;
                    std::mem::swap(buffer, &mut to_hash);
                    let wakers = std::mem::take(wakers);
                    let shutdown = *shutdown;
                    notified = hasher_buffer.1.notified();
                    drop(guard);
                    if to_hash.len() > 128 * 1024
                    /* 128 KiB */
                    {
                        hasher.update_rayon(&to_hash);
                    } else {
                        hasher.update(&to_hash);
                    }
                    to_hash.truncate(0);
                    for waker in wakers {
                        waker.wake();
                    }
                    !shutdown && to_hash.len() == 0
                } {
                    notified.await;
                }
                hasher.finalize()
            })
            .into(),
            buffer,
            capacity,
        }
    }

    pub async fn finalize(mut self) -> Result<blake3::Hash, Error> {
        self.shutdown().await?;
        self.hasher.await.with_kind(ErrorKind::Unknown)
    }
}
impl AsyncWrite for ParallelBlake3Writer {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let this = self.project();
        let mut guard = this.buffer.0.lock().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::Other, eyre!("hashing thread panicked"))
        })?;
        let (buffer, wakers, shutdown) = &mut *guard;
        if !*shutdown {
            if buffer.len() < *this.capacity {
                let to_write = std::cmp::min(*this.capacity - buffer.len(), buf.len());
                buffer.extend_from_slice(&buf[0..to_write]);
                if buffer.len() >= *this.capacity / 2 {
                    this.buffer.1.notify_waiters();
                }
                Poll::Ready(Ok(to_write))
            } else {
                wakers.push(cx.waker().clone());
                Poll::Pending
            }
        } else {
            Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                eyre!("write after shutdown"),
            )))
        }
    }
    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let this = self.project();
        let mut guard = this.buffer.0.lock().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::Other, eyre!("hashing thread panicked"))
        })?;
        let (buffer, wakers, _) = &mut *guard;
        if buffer.is_empty() {
            Poll::Ready(Ok(()))
        } else {
            wakers.push(cx.waker().clone());
            this.buffer.1.notify_waiters();
            Poll::Pending
        }
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        futures::ready!(self.as_mut().poll_flush(cx)?);
        let this = self.project();
        let mut guard = this.buffer.0.lock().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::Other, eyre!("hashing thread panicked"))
        })?;
        let (buffer, wakers, shutdown) = &mut *guard;
        if *shutdown && buffer.len() == 0 {
            return Poll::Ready(Ok(()));
        }
        wakers.push(cx.waker().clone());
        *shutdown = true;
        this.buffer.1.notify_waiters();
        Poll::Pending
    }
}