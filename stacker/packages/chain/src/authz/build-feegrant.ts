import {
  AllowedMsgAllowance,
  BasicAllowance,
  MsgGrantAllowance
} from "@initia/initia.js";

export type BuildFeeGrantInput = {
  granter: string;
  grantee: string;
  spendLimit: {
    denom: string;
    amount: string;
  };
  expiresAt: Date;
  allowedMessages?: string[];
};

export function buildFeeGrant(input: BuildFeeGrantInput) {
  const allowance = new AllowedMsgAllowance(
    new BasicAllowance(
      {
        [input.spendLimit.denom]: input.spendLimit.amount
      },
      input.expiresAt
    ),
    input.allowedMessages ?? ["/cosmos.authz.v1beta1.MsgExec"]
  );

  return new MsgGrantAllowance(input.granter, input.grantee, allowance);
}
