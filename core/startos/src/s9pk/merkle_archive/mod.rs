use std::path::Path;

use blake3::Hash;
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use imbl_value::InternedString;
use sha2::{Digest, Sha512};
use tokio::io::AsyncRead;

use crate::prelude::*;
use crate::s9pk::merkle_archive::directory_contents::DirectoryContents;
use crate::s9pk::merkle_archive::file_contents::FileContents;
use crate::s9pk::merkle_archive::sink::Sink;
use crate::s9pk::merkle_archive::source::{ArchiveSource, DynFileSource, FileSource, Section};
use crate::s9pk::merkle_archive::write_queue::WriteQueue;

pub mod directory_contents;
pub mod file_contents;
pub mod hash;
pub mod sink;
pub mod source;
#[cfg(test)]
mod test;
pub mod varint;
pub mod write_queue;

#[derive(Debug, Clone)]
enum Signer {
    Signed(VerifyingKey, Signature, u64, InternedString),
    Signer(SigningKey, InternedString),
}

#[derive(Debug, Clone)]
pub struct MerkleArchive<S> {
    signer: Signer,
    contents: DirectoryContents<S>,
}
impl<S> MerkleArchive<S> {
    pub fn new(contents: DirectoryContents<S>, signer: SigningKey, context: &str) -> Self {
        Self {
            signer: Signer::Signer(signer, context.into()),
            contents,
        }
    }
    pub fn signer(&self) -> VerifyingKey {
        match &self.signer {
            Signer::Signed(k, _, _, _) => *k,
            Signer::Signer(k, _) => k.verifying_key(),
        }
    }
    pub const fn header_size() -> u64 {
        32 // pubkey
                 + 64 // signature
                 + 32 // sighash
                 + 8 // size
                 + DirectoryContents::<Section<S>>::header_size()
    }
    pub fn contents(&self) -> &DirectoryContents<S> {
        &self.contents
    }
    pub fn contents_mut(&mut self) -> &mut DirectoryContents<S> {
        &mut self.contents
    }
    pub fn set_signer(&mut self, key: SigningKey, context: &str) {
        self.signer = Signer::Signer(key, context.into());
    }
    pub fn sort_by(
        &mut self,
        sort_by: impl Fn(&str, &str) -> std::cmp::Ordering + Send + Sync + 'static,
    ) {
        self.contents.sort_by(sort_by)
    }
}
impl<S: ArchiveSource> MerkleArchive<Section<S>> {
    #[instrument(skip_all)]
    pub async fn deserialize(
        source: &S,
        context: &str,
        header: &mut (impl AsyncRead + Unpin + Send),
    ) -> Result<Self, Error> {
        use tokio::io::AsyncReadExt;

        let mut pubkey = [0u8; 32];
        header.read_exact(&mut pubkey).await?;
        let pubkey = VerifyingKey::from_bytes(&pubkey)?;

        let mut signature = [0u8; 64];
        header.read_exact(&mut signature).await?;
        let signature = Signature::from_bytes(&signature);

        let mut sighash = [0u8; 32];
        header.read_exact(&mut sighash).await?;
        let sighash = Hash::from_bytes(sighash);

        let mut max_size = [0u8; 8];
        header.read_exact(&mut max_size).await?;
        let max_size = u64::from_be_bytes(max_size);

        pubkey.verify_prehashed_strict(
            Sha512::new_with_prefix(sighash.as_bytes()).chain_update(&u64::to_be_bytes(max_size)),
            Some(context.as_bytes()),
            &signature,
        )?;

        let contents = DirectoryContents::deserialize(source, header, (sighash, max_size)).await?;

        Ok(Self {
            signer: Signer::Signed(pubkey, signature, max_size, context.into()),
            contents,
        })
    }
}
impl<S: FileSource> MerkleArchive<S> {
    pub async fn update_hashes(&mut self, only_missing: bool) -> Result<(), Error> {
        self.contents.update_hashes(only_missing).await
    }
    pub fn filter(&mut self, filter: impl Fn(&Path) -> bool) -> Result<(), Error> {
        self.contents.filter(filter)
    }
    #[instrument(skip_all)]
    pub async fn serialize<W: Sink>(&self, w: &mut W, verify: bool) -> Result<(), Error> {
        use tokio::io::AsyncWriteExt;

        let sighash = self.contents.sighash().await?;
        let size = self.contents.toc_size();

        let (pubkey, signature, max_size) = match &self.signer {
            Signer::Signed(pubkey, signature, max_size, _) => (*pubkey, *signature, *max_size),
            Signer::Signer(s, context) => (
                s.into(),
                ed25519_dalek::SigningKey::sign_prehashed(
                    s,
                    Sha512::new_with_prefix(sighash.as_bytes())
                        .chain_update(&u64::to_be_bytes(size)),
                    Some(context.as_bytes()),
                )?,
                size,
            ),
        };

        w.write_all(pubkey.as_bytes()).await?;
        w.write_all(&signature.to_bytes()).await?;
        w.write_all(sighash.as_bytes()).await?;
        w.write_all(&u64::to_be_bytes(max_size)).await?;
        let mut next_pos = w.current_position().await?;
        next_pos += DirectoryContents::<S>::header_size();
        self.contents.serialize_header(next_pos, w).await?;
        next_pos += self.contents.toc_size();
        let mut queue = WriteQueue::new(next_pos);
        self.contents.serialize_toc(&mut queue, w).await?;
        queue.serialize(w, verify).await?;
        Ok(())
    }
    pub fn into_dyn(self) -> MerkleArchive<DynFileSource> {
        MerkleArchive {
            signer: self.signer,
            contents: self.contents.into_dyn(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry<S> {
    hash: Option<(Hash, u64)>,
    contents: EntryContents<S>,
}
impl<S> Entry<S> {
    pub fn new(contents: EntryContents<S>) -> Self {
        Self {
            hash: None,
            contents,
        }
    }
    pub fn file(source: S) -> Self {
        Self::new(EntryContents::File(FileContents::new(source)))
    }
    pub fn hash(&self) -> Option<(Hash, u64)> {
        self.hash
    }
    pub fn as_contents(&self) -> &EntryContents<S> {
        &self.contents
    }
    pub fn as_file(&self) -> Option<&FileContents<S>> {
        match self.as_contents() {
            EntryContents::File(f) => Some(f),
            _ => None,
        }
    }
    pub fn as_directory(&self) -> Option<&DirectoryContents<S>> {
        match self.as_contents() {
            EntryContents::Directory(d) => Some(d),
            _ => None,
        }
    }
    pub fn as_contents_mut(&mut self) -> &mut EntryContents<S> {
        self.hash = None;
        &mut self.contents
    }
    pub fn into_contents(self) -> EntryContents<S> {
        self.contents
    }
    pub fn into_file(self) -> Option<FileContents<S>> {
        match self.into_contents() {
            EntryContents::File(f) => Some(f),
            _ => None,
        }
    }
    pub fn into_directory(self) -> Option<DirectoryContents<S>> {
        match self.into_contents() {
            EntryContents::Directory(d) => Some(d),
            _ => None,
        }
    }
    pub fn header_size(&self) -> u64 {
        32 // hash
        + 8 // size: u64 BE
        + self.contents.header_size()
    }
}
impl<S: Clone> Entry<S> {}
impl<S: ArchiveSource> Entry<Section<S>> {
    #[instrument(skip_all)]
    pub async fn deserialize(
        source: &S,
        header: &mut (impl AsyncRead + Unpin + Send),
    ) -> Result<Self, Error> {
        use tokio::io::AsyncReadExt;

        let mut hash = [0u8; 32];
        header.read_exact(&mut hash).await?;
        let hash = Hash::from_bytes(hash);

        let mut size = [0u8; 8];
        header.read_exact(&mut size).await?;
        let size = u64::from_be_bytes(size);

        let contents = EntryContents::deserialize(source, header, (hash, size)).await?;

        Ok(Self {
            hash: Some((hash, size)),
            contents,
        })
    }
}
impl<S: FileSource> Entry<S> {
    pub fn filter(&mut self, filter: impl Fn(&Path) -> bool) -> Result<(), Error> {
        if let EntryContents::Directory(d) = &mut self.contents {
            d.filter(filter)?;
        }
        Ok(())
    }
    pub async fn read_file_to_vec(&self) -> Result<Vec<u8>, Error> {
        match self.as_contents() {
            EntryContents::File(f) => Ok(f.to_vec(self.hash).await?),
            EntryContents::Directory(_) => Err(Error::new(
                eyre!("expected file, found directory"),
                ErrorKind::ParseS9pk,
            )),
            EntryContents::Missing => {
                Err(Error::new(eyre!("entry is missing"), ErrorKind::ParseS9pk))
            }
        }
    }
    pub async fn to_missing(&self) -> Result<Self, Error> {
        let hash = if let Some(hash) = self.hash {
            hash
        } else {
            self.contents.hash().await?
        };
        Ok(Self {
            hash: Some(hash),
            contents: EntryContents::Missing,
        })
    }
    pub async fn update_hash(&mut self, only_missing: bool) -> Result<(), Error> {
        if let EntryContents::Directory(d) = &mut self.contents {
            d.update_hashes(only_missing).await?;
        }
        self.hash = Some(self.contents.hash().await?);
        Ok(())
    }
    #[instrument(skip_all)]
    pub async fn serialize_header<W: Sink>(
        &self,
        position: u64,
        w: &mut W,
    ) -> Result<Option<u64>, Error> {
        use tokio::io::AsyncWriteExt;

        let (hash, size) = if let Some(hash) = self.hash {
            hash
        } else {
            self.contents.hash().await?
        };
        w.write_all(hash.as_bytes()).await?;
        w.write_all(&u64::to_be_bytes(size)).await?;
        self.contents.serialize_header(position, w).await
    }
    pub fn into_dyn(self) -> Entry<DynFileSource> {
        Entry {
            hash: self.hash,
            contents: self.contents.into_dyn(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum EntryContents<S> {
    Missing,
    File(FileContents<S>),
    Directory(DirectoryContents<S>),
}
impl<S> EntryContents<S> {
    fn type_id(&self) -> u8 {
        match self {
            Self::Missing => 0,
            Self::File(_) => 1,
            Self::Directory(_) => 2,
        }
    }
    pub fn header_size(&self) -> u64 {
        1 // type
        + match self {
            Self::Missing => 0,
            Self::File(_) => FileContents::<S>::header_size(),
            Self::Directory(_) => DirectoryContents::<S>::header_size(),
        }
    }
    pub fn is_dir(&self) -> bool {
        matches!(self, &EntryContents::Directory(_))
    }
}
impl<S: ArchiveSource> EntryContents<Section<S>> {
    #[instrument(skip_all)]
    pub async fn deserialize(
        source: &S,
        header: &mut (impl AsyncRead + Unpin + Send),
        (hash, size): (Hash, u64),
    ) -> Result<Self, Error> {
        use tokio::io::AsyncReadExt;

        let mut type_id = [0u8];
        header.read_exact(&mut type_id).await?;
        match type_id[0] {
            0 => Ok(Self::Missing),
            1 => Ok(Self::File(
                FileContents::deserialize(source, header, size).await?,
            )),
            2 => Ok(Self::Directory(
                DirectoryContents::deserialize(source, header, (hash, size)).await?,
            )),
            id => Err(Error::new(
                eyre!("Unknown type id {id} found in MerkleArchive"),
                ErrorKind::ParseS9pk,
            )),
        }
    }
}
impl<S: FileSource> EntryContents<S> {
    pub async fn hash(&self) -> Result<(Hash, u64), Error> {
        match self {
            Self::Missing => Err(Error::new(
                eyre!("Cannot compute hash of missing file"),
                ErrorKind::Pack,
            )),
            Self::File(f) => f.hash().await,
            Self::Directory(d) => Ok((d.sighash().await?, d.toc_size())),
        }
    }
    #[instrument(skip_all)]
    pub async fn serialize_header<W: Sink>(
        &self,
        position: u64,
        w: &mut W,
    ) -> Result<Option<u64>, Error> {
        use tokio::io::AsyncWriteExt;

        w.write_all(&[self.type_id()]).await?;
        Ok(match self {
            Self::Missing => None,
            Self::File(f) => Some(f.serialize_header(position, w).await?),
            Self::Directory(d) => Some(d.serialize_header(position, w).await?),
        })
    }
    pub fn into_dyn(self) -> EntryContents<DynFileSource> {
        match self {
            Self::Missing => EntryContents::Missing,
            Self::File(f) => EntryContents::File(f.into_dyn()),
            Self::Directory(d) => EntryContents::Directory(d.into_dyn()),
        }
    }
}