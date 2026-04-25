import { MsgExecute } from "@initia/initia.js";

export type BuildDirectSingleAssetProvideDelegateInput = {
  userAddress: string;
  moduleAddress: string;
  moduleName: string;
  args: string[];
};

export function buildDirectSingleAssetProvideDelegate(
  input: BuildDirectSingleAssetProvideDelegateInput
) {
  return new MsgExecute(
    input.userAddress,
    input.moduleAddress,
    input.moduleName,
    "single_asset_provide_delegate",
    [],
    input.args
  );
}
