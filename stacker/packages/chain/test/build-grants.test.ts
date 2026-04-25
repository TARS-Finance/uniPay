import { describe, expect, it } from "vitest";
import { buildFeeGrant } from "../src/authz/build-feegrant.js";
import { buildMoveGrant } from "../src/authz/build-move-grant.js";
import { buildStakeGrant } from "../src/authz/build-stake-grant.js";

const granter = "init1granter";
const grantee = "init1keeper";
const expiration = new Date("2026-04-30T00:00:00.000Z");

describe("grant builders", () => {
  it("builds a narrow move execute grant", () => {
    const grant = buildMoveGrant({
      granter,
      grantee,
      moduleAddress: "0x1",
      moduleName: "dex",
      functionNames: ["single_asset_provide_liquidity_script"],
      expiresAt: expiration
    });

    expect(grant.toData()).toEqual({
      "@type": "/cosmos.authz.v1beta1.MsgGrant",
      granter,
      grantee,
      grant: {
        authorization: {
          "@type": "/initia.move.v1.ExecuteAuthorization",
          items: [
            {
              module_address: "0x1",
              module_name: "dex",
              function_names: ["single_asset_provide_liquidity_script"]
            }
          ]
        },
        expiration: "2026-04-30T00:00:00.000Z"
      }
    });
  });

  it("builds a delegate-only staking grant", () => {
    const grant = buildStakeGrant({
      granter,
      grantee,
      validatorAddress: "initvaloper1validator",
      maxTokens: {
        denom: "uinit",
        amount: "1000"
      },
      expiresAt: expiration
    });

    expect(grant.toData()).toEqual({
      "@type": "/cosmos.authz.v1beta1.MsgGrant",
      granter,
      grantee,
      grant: {
        authorization: {
          "@type": "/initia.mstaking.v1.StakeAuthorization",
          max_tokens: [{ denom: "uinit", amount: "1000" }],
          allow_list: { address: ["initvaloper1validator"] },
          deny_list: { address: [] },
          authorization_type: "AUTHORIZATION_TYPE_DELEGATE"
        },
        expiration: "2026-04-30T00:00:00.000Z"
      }
    });
  });

  it("builds a feegrant restricted to authz execution", () => {
    const grant = buildFeeGrant({
      granter,
      grantee,
      spendLimit: {
        denom: "uinit",
        amount: "2500"
      },
      expiresAt: expiration
    });

    expect(grant.toData()).toEqual({
      "@type": "/cosmos.feegrant.v1beta1.MsgGrantAllowance",
      granter,
      grantee,
      allowance: {
        "@type": "/cosmos.feegrant.v1beta1.AllowedMsgAllowance",
        allowance: {
          "@type": "/cosmos.feegrant.v1beta1.BasicAllowance",
          spend_limit: [{ denom: "uinit", amount: "2500" }],
          expiration: "2026-04-30T00:00:00.000Z"
        },
        allowed_messages: ["/cosmos.authz.v1beta1.MsgExec"]
      }
    });
  });
});
