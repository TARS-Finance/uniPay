import { MsgExecute } from "@initia/initia.js";
import { encodeAuthorizedMsgExec } from "../authz/encode-msg-exec.js";

export type ProvideSingleAssetLiquidityInput = {
  grantee: string;
  userAddress: string;
  moduleAddress: string;
  moduleName: string;
  typeArgs?: string[];
  args: string[];
};

export function provideSingleAssetLiquidity(
  input: ProvideSingleAssetLiquidityInput
) {
  const msg = new MsgExecute(
    input.userAddress,
    input.moduleAddress,
    input.moduleName,
    "single_asset_provide_liquidity_script",
    input.typeArgs ?? [],
    input.args
  );

  return encodeAuthorizedMsgExec({
    grantee: input.grantee,
    msgs: [msg]
  });
}
