import { bcs } from "@initia/initia.js";
import { describe, expect, it, vi } from "vitest";
import { createLiveKeeperChainClient } from "../src/index.js";

const pairObjectId = `0x${"1".repeat(64)}`;
const coinMetadataObjectId = `0x${"2".repeat(64)}`;
const merchantAddress = "init18f735agmd8zav9lrtnregkqn7eu4wc8cnanpql";
const validatorAddress = "initvaloper1cduny8wdjupu2lhya9npc9j4x5ytn05kt36x0c";

describe("live keeper chain client direct mode", () => {
  it("signs a direct MsgExecute for single-asset provide+delegate", async () => {
    const bondedLockedDelegations = vi
      .fn()
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([
        {
          metadata: pairObjectId,
          validator: validatorAddress,
          locked_share: "828440",
          amount: "828440",
          release_time: "1776970386",
        },
      ]);
    const broadcast = vi.fn(async () => ({
      txhash: "provide-delegate-hash",
      raw_log: "",
      logs: [],
    }));
    const simulate = vi.fn(async () => ({
      result: {
        events: [
          {
            type: "move",
            attributes: [
              { key: "type_tag", value: "0x1::dex::ProvideEvent" },
              { key: "liquidity_token", value: pairObjectId },
              { key: "liquidity", value: "828440" },
            ],
          },
        ],
      },
    }));
    const txInfo = vi.fn(async () => ({
      events: [],
    }));
    const createAndSignTx = vi.fn(async (input: unknown) => input);

    const client = createLiveKeeperChainClient({
      lcdUrl: "https://rest.testnet.initia.xyz",
      privateKey: "1".repeat(64),
      keeperAddress: merchantAddress,
      executionMode: "direct",
      restClient: {
        bank: {
          balanceByDenom: vi.fn(async () => ({ amount: "0", denom: "ulp" })),
        },
        move: {
          metadata: vi.fn(async () => coinMetadataObjectId),
          viewFunction: bondedLockedDelegations,
        },
        mstaking: {
          delegation: vi.fn(),
        },
        tx: {
          simulate,
          broadcast,
          txInfo,
        },
      },
      wallet: {
        accAddress: merchantAddress,
        createAndSignTx,
        sequence: vi.fn(async () => 7),
      },
    });

    await expect(
      client.singleAssetProvideDelegate({
        userAddress: merchantAddress,
        targetPoolId: pairObjectId,
        inputDenom: "uusdc",
        lpDenom: "ulp",
        amount: "1000000",
        maxSlippageBps: "100",
        moduleAddress:
          "0x81c3ea419d2fd3a27971021d9dd3cc708def05e5d6a09d39b2f1f9ba18312264",
        moduleName: "lock_staking",
        releaseTime: "1776970386",
        validatorAddress,
      })
    ).resolves.toMatchObject({
      txHash: "provide-delegate-hash",
      lpAmount: "828440",
    });

    const txInput = createAndSignTx.mock.calls[0]?.[0] as {
      msgs: Array<{
        toData(): {
          "@type": string;
          args: string[];
        };
      }>;
    };

    expect(txInput.msgs[0]?.toData()["@type"]).toBe("/initia.move.v1.MsgExecute");
    expect(txInput.msgs[0]?.toData().args[4]).toBe(
      bcs.u64().serialize(1776970386n).toBase64()
    );
    expect(txInput.msgs[0]?.toData().args[5]).toBe(
      bcs.string().serialize(validatorAddress).toBase64()
    );
  });
});
