import { bcs } from "@initia/initia.js";
import { describe, expect, it, vi } from "vitest";
import { createLiveKeeperChainClient } from "../src/index.js";

const pairObjectId = `0x${"1".repeat(64)}`;
const coinMetadataObjectId = `0x${"2".repeat(64)}`;
const userAddress = "init18f735agmd8zav9lrtnregkqn7eu4wc8cnanpql";
const validatorAddress = "initvaloper1cduny8wdjupu2lhya9npc9j4x5ytn05kt36x0c";

describe("live keeper chain client", () => {
  it("reads input, lp, and delegated balances from Initia REST queries", async () => {
    const balanceByDenom = vi.fn(async (_address: string, denom: string) => {
      if (denom === "usdc") {
        return { amount: "500", denom };
      }

      if (denom === "ulp") {
        return { amount: "25", denom };
      }

      return undefined;
    });

    const client = createLiveKeeperChainClient({
      lcdUrl: "https://rest.testnet.initia.xyz",
      privateKey: "1".repeat(64),
      keeperAddress: "init1keeperaddress",
      restClient: {
        bank: { balanceByDenom },
        move: {
          metadata: vi.fn(async () => coinMetadataObjectId)
        },
        mstaking: {
          delegation: vi.fn(async () => ({
            balance: {
              get: (denom: string) =>
                denom === "ulp" ? { amount: "18", denom } : undefined
            }
          }))
        },
        tx: {
          simulate: vi.fn(),
          broadcast: vi.fn(),
          txInfo: vi.fn()
        }
      },
      wallet: {
        accAddress: "init1keeperaddress",
        createAndSignTx: vi.fn(),
        sequence: vi.fn(async () => 7)
      }
    });

    await expect(
      client.getInputBalance({
        userAddress,
        denom: "usdc"
      })
    ).resolves.toBe("500");
    await expect(
      client.getLpBalance({
        userAddress,
        lpDenom: "ulp"
      })
    ).resolves.toBe("25");
    await expect(
      client.getDelegatedLpBalance({
        userAddress,
        validatorAddress,
        lpDenom: "ulp"
      })
    ).resolves.toBe("18");
  });

  it("reads bonded locked lp balance from the lock-staking view", async () => {
    const client = createLiveKeeperChainClient({
      lcdUrl: "https://rest.testnet.initia.xyz",
      privateKey: "1".repeat(64),
      keeperAddress: "init1keeperaddress",
      restClient: {
        bank: {
          balanceByDenom: vi.fn()
        },
        move: {
          metadata: vi.fn(async () => coinMetadataObjectId),
          viewFunction: vi.fn(async () => [
            {
          metadata: pairObjectId,
          validator: validatorAddress,
          locked_share: "250",
          amount: "250",
          release_time: "1777057735"
            },
            {
              metadata: pairObjectId,
              validator: "initvaloper1othervalidator",
              locked_share: "999",
              amount: "999",
              release_time: "1777057735"
            }
          ])
        },
        mstaking: {
          delegation: vi.fn()
        },
        tx: {
          simulate: vi.fn(),
          broadcast: vi.fn(),
          txInfo: vi.fn()
        }
      },
      wallet: {
        accAddress: "init1keeperaddress",
        createAndSignTx: vi.fn(),
        sequence: vi.fn(async () => 7)
      }
    });

    await expect(
      client.getBondedLockedLpBalance({
        userAddress,
        targetPoolId: pairObjectId,
        validatorAddress,
        moduleAddress: "0xlock",
        moduleName: "lock_staking"
      })
    ).resolves.toBe("250");
  });

  it("signs and broadcasts an authz-wrapped provide tx and returns the lp delta", async () => {
    const balanceByDenom = vi
      .fn()
      .mockResolvedValueOnce({ amount: "10", denom: "ulp" })
      .mockResolvedValueOnce({ amount: "25", denom: "ulp" });
    const broadcast = vi.fn(async () => ({
      txhash: "provide-hash",
      raw_log: "",
      logs: []
    }));
    const simulate = vi.fn(async () => ({
      result: {
        events: [
          {
            type: "move",
            attributes: [
              { key: "type_tag", value: "0x1::dex::ProvideEvent" },
              { key: "liquidity_token", value: pairObjectId },
              { key: "liquidity", value: "20" }
            ]
          }
        ]
      }
    }));
    const createAndSignTx = vi.fn(async (input: unknown) => input);

    const client = createLiveKeeperChainClient({
      lcdUrl: "https://rest.testnet.initia.xyz",
      privateKey: "1".repeat(64),
      keeperAddress: "init1keeperaddress",
      restClient: {
        bank: { balanceByDenom },
        move: {
          metadata: vi.fn(async () => coinMetadataObjectId)
        },
        mstaking: {
          delegation: vi.fn()
        },
        tx: {
          simulate,
          broadcast,
          txInfo: vi.fn()
        }
      },
      wallet: {
        accAddress: "init1keeperaddress",
        createAndSignTx,
        sequence: vi.fn(async () => 7)
      }
    });

    await expect(
      client.provideSingleAssetLiquidity({
        userAddress,
        targetPoolId: pairObjectId,
        inputDenom: "usdc",
        lpDenom: "ulp",
        amount: "250",
        maxSlippageBps: "100",
        moduleAddress: "0x1",
        moduleName: "dex"
      })
    ).resolves.toEqual({
      txHash: "provide-hash",
      lpAmount: "15"
    });

    const provideTxInput = createAndSignTx.mock.calls[0]?.[0] as {
      msgs: Array<{
        toData(): {
          "@type": string;
          msgs: Array<{ args: string[] }>;
        };
      }>;
    };

    expect(createAndSignTx).toHaveBeenCalledTimes(1);
    expect(simulate).toHaveBeenCalledTimes(1);
    expect(provideTxInput.msgs[0]?.toData()["@type"]).toBe(
      "/cosmos.authz.v1beta1.MsgExec"
    );
    expect(
      provideTxInput.msgs[0]?.toData().msgs[0]?.args[3]
    ).toBe(bcs.option(bcs.u64()).serialize(19n).toBase64());
    expect(broadcast).toHaveBeenCalledTimes(1);
  });

  it("signs and broadcasts an authz-wrapped single-asset provide+delegate tx", async () => {
    const bondedLockedDelegations = vi
      .fn()
      .mockResolvedValueOnce([
        {
          metadata: pairObjectId,
          validator: validatorAddress,
          locked_share: "10",
          amount: "10",
          release_time: "1776970386"
        }
      ])
      .mockResolvedValueOnce([
        {
          metadata: pairObjectId,
          validator: validatorAddress,
          locked_share: "25",
          amount: "25",
          release_time: "1776970386"
        }
      ]);
    const broadcast = vi.fn(async () => ({
      txhash: "provide-delegate-hash",
      raw_log: "",
      logs: []
    }));
    const txInfo = vi.fn(async () => ({
      events: [
        {
          type: "move",
          attributes: [
            {
              key: "type_tag",
              value: "0xlock::lock_staking::DepositDelegationEvent"
            },
            { key: "staking_account", value: `0x${"4".repeat(64)}` },
            { key: "metadata", value: pairObjectId },
            { key: "release_time", value: "1776970386" },
            { key: "validator", value: validatorAddress },
            { key: "locked_share", value: "25" }
          ]
        }
      ]
    }));
    const simulate = vi.fn(async () => ({
      result: {
        events: [
          {
            type: "move",
            attributes: [
              { key: "type_tag", value: "0x1::dex::ProvideEvent" },
              { key: "liquidity_token", value: pairObjectId },
              { key: "liquidity", value: "20" }
            ]
          }
        ]
      }
    }));
    const createAndSignTx = vi.fn(async (input: unknown) => input);

    const client = createLiveKeeperChainClient({
      lcdUrl: "https://rest.testnet.initia.xyz",
      privateKey: "1".repeat(64),
      keeperAddress: "init1keeperaddress",
      restClient: {
        bank: {
          balanceByDenom: vi.fn(async () => ({ amount: "0", denom: "ulp" }))
        },
        move: {
          metadata: vi.fn(async () => coinMetadataObjectId),
          viewFunction: bondedLockedDelegations
        },
        mstaking: {
          delegation: vi.fn()
        },
        tx: {
          simulate,
          broadcast,
          txInfo
        }
      },
      wallet: {
        accAddress: "init1keeperaddress",
        createAndSignTx,
        sequence: vi.fn(async () => 7)
      }
    });

    await expect(
      client.singleAssetProvideDelegate({
        userAddress,
        targetPoolId: pairObjectId,
        inputDenom: "usdc",
        lpDenom: "ulp",
        amount: "250",
        maxSlippageBps: "100",
        moduleAddress: "0xlock",
        moduleName: "lock_staking",
        releaseTime: "1776970386",
        validatorAddress
      })
    ).resolves.toEqual({
      txHash: "provide-delegate-hash",
      lpAmount: "15",
      rewardSnapshot: {
        kind: "bonded-locked",
        stakingAccount: `0x${"4".repeat(64)}`,
        metadata: pairObjectId,
        releaseTime: "1776970386",
        releaseTimeIso: "2026-04-23T18:53:06.000Z",
        validatorAddress,
        lockedShare: "25"
      }
    });

    const provideDelegateTxInput = createAndSignTx.mock.calls[0]?.[0] as {
      msgs: Array<{
        toData(): {
          "@type": string;
          msgs: Array<{ args: string[] }>;
        };
      }>;
    };

    expect(simulate).toHaveBeenCalledTimes(1);
    expect(provideDelegateTxInput.msgs[0]?.toData()["@type"]).toBe(
      "/cosmos.authz.v1beta1.MsgExec"
    );
    expect(
      provideDelegateTxInput.msgs[0]?.toData().msgs[0]?.args[4]
    ).toBe(bcs.u64().serialize(1776970386n).toBase64());
    expect(
      provideDelegateTxInput.msgs[0]?.toData().msgs[0]?.args[5]
    ).toBe(bcs.string().serialize(validatorAddress).toBase64());
    expect(bondedLockedDelegations).toHaveBeenCalledTimes(2);
    expect(txInfo).toHaveBeenCalledWith("provide-delegate-hash");
    expect(broadcast).toHaveBeenCalledTimes(1);
  });

  it("refuses to broadcast a provide tx when simulation cannot derive an lp quote", async () => {
    const broadcast = vi.fn(async () => ({
      txhash: "provide-hash",
      raw_log: "",
      logs: []
    }));
    const simulate = vi.fn(async () => ({
      result: {
        events: [
          {
            type: "move",
            attributes: [
              { key: "type_tag", value: "0x1::dex::ProvideEvent" },
              { key: "liquidity_token", value: `0x${"3".repeat(64)}` },
              { key: "liquidity", value: "20" }
            ]
          }
        ]
      }
    }));

    const client = createLiveKeeperChainClient({
      lcdUrl: "https://rest.testnet.initia.xyz",
      privateKey: "1".repeat(64),
      keeperAddress: "init1keeperaddress",
      restClient: {
        bank: {
          balanceByDenom: vi.fn(async () => ({ amount: "10", denom: "ulp" }))
        },
        move: {
          metadata: vi.fn(async () => coinMetadataObjectId)
        },
        mstaking: {
          delegation: vi.fn()
        },
        tx: {
          simulate,
          broadcast,
          txInfo: vi.fn()
        }
      },
      wallet: {
        accAddress: "init1keeperaddress",
        createAndSignTx: vi.fn(async (input: unknown) => input),
        sequence: vi.fn(async () => 7)
      }
    });

    await expect(
      client.provideSingleAssetLiquidity({
        userAddress,
        targetPoolId: pairObjectId,
        inputDenom: "usdc",
        lpDenom: "ulp",
        amount: "250",
        maxSlippageBps: "100",
        moduleAddress: "0x1",
        moduleName: "dex"
      })
    ).rejects.toThrow(/lp quote/i);

    expect(simulate).toHaveBeenCalledTimes(1);
    expect(broadcast).not.toHaveBeenCalled();
  });

  it("signs and broadcasts an authz-wrapped delegate tx", async () => {
    const broadcast = vi.fn(async () => ({
      txhash: "delegate-hash",
      raw_log: "",
      logs: []
    }));
    const createAndSignTx = vi.fn(async (input: unknown) => input);

    const client = createLiveKeeperChainClient({
      lcdUrl: "https://rest.testnet.initia.xyz",
      privateKey: "1".repeat(64),
      keeperAddress: "init1keeperaddress",
      restClient: {
        bank: {
          balanceByDenom: vi.fn()
        },
        move: {
          metadata: vi.fn(async () => coinMetadataObjectId)
        },
        mstaking: {
          delegation: vi.fn()
        },
        tx: {
          simulate: vi.fn(),
          broadcast,
          txInfo: vi.fn()
        }
      },
      wallet: {
        accAddress: "init1keeperaddress",
        createAndSignTx,
        sequence: vi.fn(async () => 7)
      }
    });

    await expect(
      client.delegateLp({
        userAddress,
        validatorAddress,
        lpDenom: "ulp",
        amount: "15"
      })
    ).resolves.toEqual({
      txHash: "delegate-hash"
    });

    const delegateTxInput = createAndSignTx.mock.calls[0]?.[0] as {
      msgs: Array<{ toData(): { "@type": string } }>;
    };

    expect(delegateTxInput.msgs[0]?.toData()["@type"]).toBe(
      "/cosmos.authz.v1beta1.MsgExec"
    );
  });

  it("fails fast when the derived wallet address does not match the configured keeper", () => {
    expect(() =>
      createLiveKeeperChainClient({
        lcdUrl: "https://rest.testnet.initia.xyz",
        privateKey: "1".repeat(64),
        keeperAddress: "init1configuredkeeper",
        restClient: {
          bank: {
            balanceByDenom: vi.fn()
          },
          move: {
            metadata: vi.fn(async () => coinMetadataObjectId)
          },
          mstaking: {
            delegation: vi.fn()
          },
          tx: {
            simulate: vi.fn(),
            broadcast: vi.fn(),
            txInfo: vi.fn()
          }
        },
        wallet: {
          accAddress: "init1differentkeeper",
          createAndSignTx: vi.fn(),
          sequence: vi.fn()
        }
      })
    ).toThrow(/keeper address/i);
  });
});
