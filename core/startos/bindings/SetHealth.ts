// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { HealthCheckId } from "./HealthCheckId";

export type SetHealth = { id: HealthCheckId; name: string } & (
  | { result: "success"; message: string | null }
  | { result: "disabled"; message: string | null }
  | { result: "starting"; message: string | null }
  | { result: "loading"; message: string }
  | { result: "failure"; message: string }
);
