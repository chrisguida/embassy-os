// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { BackupProgress } from "./BackupProgress"
import type { FullProgress } from "./FullProgress"
import type { PackageId } from "./PackageId"

export type ServerStatus = {
  backupProgress: { [key: PackageId]: BackupProgress } | null
  updated: boolean
  updateProgress: FullProgress | null
  shuttingDown: boolean
  restarting: boolean
}