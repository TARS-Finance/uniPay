import { MsgExecAuthorized } from "@initia/initia.js";
import type { Msg } from "@initia/initia.js";

export type EncodeAuthorizedMsgExecInput = {
  grantee: string;
  msgs: Msg[];
};

export function encodeAuthorizedMsgExec(input: EncodeAuthorizedMsgExecInput) {
  return new MsgExecAuthorized(input.grantee, input.msgs);
}
