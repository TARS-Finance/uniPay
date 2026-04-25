import { MsgDelegate, MsgExecute } from "@initia/initia.js";
import { describe, expect, it } from "vitest";
import { encodeAuthorizedMsgExec } from "../src/authz/encode-msg-exec.js";
import { provideSingleAssetLiquidity } from "../src/dex/provide-single-asset-liquidity.js";
import { delegateLp } from "../src/staking/delegate-lp.js";
import { buildDirectSingleAssetProvideDelegate } from "../src/vip/build-direct-single-asset-provide-delegate.js";
import { singleAssetProvideDelegate } from "../src/vip/single-asset-provide-delegate.js";

describe("execution encoders", () => {
  it("wraps a move liquidity message in MsgExec", () => {
    const message = provideSingleAssetLiquidity({
      grantee: "init1keeper",
      userAddress: "init1user",
      moduleAddress: "0x1",
      moduleName: "dex",
      args: ["YXJnMQ=="]
    });

    expect(message.toData()).toEqual({
      "@type": "/cosmos.authz.v1beta1.MsgExec",
      grantee: "init1keeper",
      msgs: [
        {
          "@type": "/initia.move.v1.MsgExecute",
          sender: "init1user",
          module_address: "0x1",
          module_name: "dex",
          function_name: "single_asset_provide_liquidity_script",
          type_args: [],
          args: ["YXJnMQ=="]
        }
      ]
    });
  });

  it("wraps an mstaking delegate message in MsgExec", () => {
    const message = delegateLp({
      grantee: "init1keeper",
      userAddress: "init1user",
      validatorAddress: "initvaloper1validator",
      lpDenom: "ulp",
      amount: "42"
    });

    expect(message.toData()).toEqual({
      "@type": "/cosmos.authz.v1beta1.MsgExec",
      grantee: "init1keeper",
      msgs: [
        {
          "@type": "/initia.mstaking.v1.MsgDelegate",
          delegator_address: "init1user",
          validator_address: "initvaloper1validator",
          amount: [{ denom: "ulp", amount: "42" }]
        }
      ]
    });
  });

  it("wraps a lock-staking single asset provide+delegate message in MsgExec", () => {
    const message = singleAssetProvideDelegate({
      grantee: "init1keeper",
      userAddress: "init1user",
      moduleAddress: "0xlock",
      moduleName: "lock_staking",
      args: ["YXJnMQ=="]
    });

    expect(message.toData()).toEqual({
      "@type": "/cosmos.authz.v1beta1.MsgExec",
      grantee: "init1keeper",
      msgs: [
        {
          "@type": "/initia.move.v1.MsgExecute",
          sender: "init1user",
          module_address: "0xlock",
          module_name: "lock_staking",
          function_name: "single_asset_provide_delegate",
          type_args: [],
          args: ["YXJnMQ=="]
        }
      ]
    });
  });

  it("builds a direct user-signed lock-staking single asset provide+delegate message", () => {
    const message = buildDirectSingleAssetProvideDelegate({
      userAddress: "init1user",
      moduleAddress: "0xlock",
      moduleName: "lock_staking",
      args: ["YXJnMQ=="]
    });

    expect(message.toData()).toEqual({
      "@type": "/initia.move.v1.MsgExecute",
      sender: "init1user",
      module_address: "0xlock",
      module_name: "lock_staking",
      function_name: "single_asset_provide_delegate",
      type_args: [],
      args: ["YXJnMQ=="]
    });
  });

  it("keeps generic authorized execution wiring aligned with the keeper model", () => {
    const message = encodeAuthorizedMsgExec({
      grantee: "init1keeper",
      msgs: [
        new MsgExecute("init1user", "0x1", "dex", "single_asset_provide_liquidity_script", [], []),
        new MsgDelegate("init1user", "initvaloper1validator", { ulp: "42" })
      ]
    });

    expect(message.toData()).toMatchObject({
      "@type": "/cosmos.authz.v1beta1.MsgExec",
      grantee: "init1keeper"
    });
    expect(message.toData().msgs).toHaveLength(2);
  });
});
