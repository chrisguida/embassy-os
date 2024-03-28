// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { AddressInfo } from "./AddressInfo";
import type { ServiceInterfaceType } from "./ServiceInterfaceType";

export type ExportServiceInterfaceParams = {
  id: string;
  name: string;
  description: string;
  hasPrimary: boolean;
  disabled: boolean;
  masked: boolean;
  addressInfo: AddressInfo;
  type: ServiceInterfaceType;
};
