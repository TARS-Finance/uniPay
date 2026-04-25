import {
  AuthorizationGrant,
  MsgGrantAuthorization,
  StakeAuthorization,
  StakeAuthorizationValidators
} from "@initia/initia.js";

export type BuildStakeGrantInput = {
  granter: string;
  grantee: string;
  validatorAddress: string;
  maxTokens: {
    denom: string;
    amount: string;
  };
  expiresAt: Date;
};

export function buildStakeGrant(input: BuildStakeGrantInput) {
  const authorization = new StakeAuthorization(
    {
      [input.maxTokens.denom]: input.maxTokens.amount
    },
    new StakeAuthorizationValidators([input.validatorAddress]),
    new StakeAuthorizationValidators([]),
    StakeAuthorization.Type.AUTHORIZATION_TYPE_DELEGATE
  );
  const grant = new AuthorizationGrant(authorization, input.expiresAt);

  return new MsgGrantAuthorization(input.granter, input.grantee, grant);
}
