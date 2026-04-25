import { buildFeeGrant, buildMoveGrant } from "@stacker/chain";
import { GrantsRepository, StrategiesRepository, UsersRepository } from "@stacker/db";
import type { ApiConfig } from "../config.js";
import type { GrantVerifier } from "./grant-verifier.js";

type ConfirmGrantResult =
  | {
      kind: "confirmed";
      strategyId: string;
      strategyStatus: string;
      grantStatus: {
        move: "active";
        staking: "not-required";
        feegrant: "active";
      };
    }
  | {
      kind: "not_found";
    }
  | {
      kind: "verification_failed";
      missing: Array<"move" | "feegrant">;
    };

export class GrantsService {
  constructor(
    private readonly grantsRepository: GrantsRepository,
    private readonly strategiesRepository: StrategiesRepository,
    private readonly usersRepository: UsersRepository,
    private readonly config: ApiConfig,
    private readonly grantVerifier: GrantVerifier
  ) {}

  async prepare(userId: string, strategyId: string) {
    const strategy = await this.strategiesRepository.findById(strategyId);
    const user = await this.usersRepository.findById(userId);

    if (!strategy || strategy.userId !== userId || !user) {
      return null;
    }

    const expiresAt = new Date(
      Date.now() + this.config.grantExpiryHours * 60 * 60 * 1000
    );
    const moveGrant = buildMoveGrant({
      granter: user.initiaAddress,
      grantee: this.config.keeperAddress,
      moduleAddress: this.config.lockStakingModuleAddress,
      moduleName: this.config.lockStakingModuleName,
      functionNames: ["single_asset_provide_delegate"],
      expiresAt
    });
    const feeGrant = buildFeeGrant({
      granter: user.initiaAddress,
      grantee: this.config.keeperAddress,
      spendLimit: {
        denom: this.config.feeDenom,
        amount: "2500"
      },
      expiresAt
    });

    await this.grantsRepository.upsertForUser({
      userId,
      keeperAddress: this.config.keeperAddress,
      moveGrantExpiresAt: expiresAt,
      stakingGrantExpiresAt: null,
      feegrantExpiresAt: expiresAt,
      moveGrantStatus: "pending",
      stakingGrantStatus: "pending",
      feegrantStatus: "pending",
      scopeJson: {
        moveGrant: moveGrant.toData(),
        stakingGrant: null,
        feeGrant: feeGrant.toData()
      }
    });

    return {
      keeperAddress: this.config.keeperAddress,
      grants: {
        move: moveGrant.toData(),
        staking: null,
        feegrant: feeGrant.toData()
      }
    };
  }

  async confirm(userId: string, strategyId: string): Promise<ConfirmGrantResult> {
    const strategy = await this.strategiesRepository.findById(strategyId);
    const existingGrant = await this.grantsRepository.findByUserId(userId);
    const user = await this.usersRepository.findById(userId);

    if (!strategy || strategy.userId !== userId || !existingGrant || !user) {
      return {
        kind: "not_found"
      };
    }

    const verification = await this.grantVerifier.verify({
      granterAddress: user.initiaAddress,
      granteeAddress: this.config.keeperAddress,
      moduleAddress: this.config.lockStakingModuleAddress,
      moduleName: this.config.lockStakingModuleName,
      functionName: "single_asset_provide_delegate",
      feeAllowedMessage: "/cosmos.authz.v1beta1.MsgExec"
    });
    const missing: Array<"move" | "feegrant"> = [];

    if (!verification.moveGrantActive) {
      missing.push("move");
    }

    if (!verification.feegrantActive) {
      missing.push("feegrant");
    }

    if (missing.length > 0) {
      return {
        kind: "verification_failed",
        missing
      };
    }

    await this.grantsRepository.upsertForUser({
      userId,
      keeperAddress: existingGrant.keeperAddress,
      moveGrantExpiresAt: existingGrant.moveGrantExpiresAt,
      stakingGrantExpiresAt: null,
      feegrantExpiresAt: existingGrant.feegrantExpiresAt,
      moveGrantStatus: "active",
      stakingGrantStatus: existingGrant.stakingGrantStatus,
      feegrantStatus: "active",
      scopeJson: existingGrant.scopeJson
    });

    const updatedStrategy = await this.strategiesRepository.updateStatus(
      strategyId,
      "active"
    );

    return {
      kind: "confirmed",
      strategyId,
      strategyStatus: updatedStrategy?.status ?? "active",
      grantStatus: {
        move: "active",
        staking: "not-required",
        feegrant: "active"
      }
    };
  }
}
