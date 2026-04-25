import { MsgExecute } from "@initia/initia.js";
import { encodeAuthorizedMsgExec } from "../authz/encode-msg-exec.js";

export type SingleAssetProvideDelegateInput = {
  grantee: string;
  userAddress: string;
  moduleAddress: string;
  moduleName: string;
  args: string[];
};

export function singleAssetProvideDelegate(
  input: SingleAssetProvideDelegateInput
) {
  return encodeAuthorizedMsgExec({
    grantee: input.grantee,
    msgs: [
      new MsgExecute(
        input.userAddress,
        input.moduleAddress,
        input.moduleName,
        "single_asset_provide_delegate",
        [],
        input.args
      )
    ]
  });
}
