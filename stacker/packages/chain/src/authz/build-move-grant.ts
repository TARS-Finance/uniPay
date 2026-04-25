import {
  AuthorizationGrant,
  ExecuteAuthorization,
  ExecuteAuthorizationItem,
  MsgGrantAuthorization
} from "@initia/initia.js";

export type BuildMoveGrantInput = {
  granter: string;
  grantee: string;
  moduleAddress: string;
  moduleName: string;
  functionNames: string[];
  expiresAt: Date;
};

export function buildMoveGrant(input: BuildMoveGrantInput) {
  const authorization = new ExecuteAuthorization([
    new ExecuteAuthorizationItem(
      input.moduleAddress,
      input.moduleName,
      input.functionNames
    )
  ]);
  const grant = new AuthorizationGrant(authorization, input.expiresAt);

  return new MsgGrantAuthorization(input.granter, input.grantee, grant);
}
