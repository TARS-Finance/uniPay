import { MsgDelegate } from "@initia/initia.js";
import { encodeAuthorizedMsgExec } from "../authz/encode-msg-exec.js";

export type DelegateLpInput = {
  grantee: string;
  userAddress: string;
  validatorAddress: string;
  lpDenom: string;
  amount: string;
};

export function delegateLp(input: DelegateLpInput) {
  const msg = new MsgDelegate(
    input.userAddress,
    input.validatorAddress,
    {
      [input.lpDenom]: input.amount
    }
  );

  return encodeAuthorizedMsgExec({
    grantee: input.grantee,
    msgs: [msg]
  });
}
