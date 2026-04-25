import {
  index,
  jsonb,
  pgEnum,
  pgTable,
  text,
  timestamp,
  uniqueIndex,
  uuid
} from "drizzle-orm/pg-core";

export const inputDenomEnum = pgEnum("input_denom", ["usdc", "iusdc", "uusdc"]);
export const strategyStatusEnum = pgEnum("strategy_status", [
  "draft",
  "grant_pending",
  "active",
  "executing",
  "partial_lp",
  "paused",
  "expired",
  "error"
]);
export const executionStatusEnum = pgEnum("execution_status", [
  "queued",
  "providing",
  "delegating",
  "simulated",
  "success",
  "failed",
  "retryable"
]);
export const grantStatusEnum = pgEnum("grant_status", [
  "pending",
  "active",
  "revoked",
  "expired"
]);
export const withdrawalStatusEnum = pgEnum("withdrawal_status", [
  "pending",
  "confirmed",
  "failed"
]);

export const users = pgTable(
  "users",
  {
    id: uuid("id").defaultRandom().primaryKey(),
    initiaAddress: text("initia_address").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true })
      .defaultNow()
      .notNull(),
    updatedAt: timestamp("updated_at", { withTimezone: true })
      .defaultNow()
      .notNull()
  },
  (table) => [uniqueIndex("users_initia_address_unique").on(table.initiaAddress)]
);

export const strategies = pgTable(
  "strategies",
  {
    id: uuid("id").defaultRandom().primaryKey(),
    userId: uuid("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    status: strategyStatusEnum("status").notNull().default("draft"),
    inputDenom: inputDenomEnum("input_denom").notNull(),
    targetPoolId: text("target_pool_id").notNull(),
    dexModuleAddress: text("dex_module_address").notNull(),
    dexModuleName: text("dex_module_name").notNull(),
    validatorAddress: text("validator_address").notNull(),
    minBalanceAmount: text("min_balance_amount").notNull(),
    maxAmountPerRun: text("max_amount_per_run").notNull(),
    maxSlippageBps: text("max_slippage_bps").notNull(),
    cooldownSeconds: text("cooldown_seconds").notNull(),
    lastExecutedAt: timestamp("last_executed_at", { withTimezone: true }),
    nextEligibleAt: timestamp("next_eligible_at", { withTimezone: true }),
    pauseReason: text("pause_reason"),
    createdAt: timestamp("created_at", { withTimezone: true })
      .defaultNow()
      .notNull(),
    updatedAt: timestamp("updated_at", { withTimezone: true })
      .defaultNow()
      .notNull()
  },
  (table) => [
    index("strategies_user_id_index").on(table.userId),
    index("strategies_status_index").on(table.status)
  ]
);

export const grants = pgTable(
  "grants",
  {
    id: uuid("id").defaultRandom().primaryKey(),
    userId: uuid("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    keeperAddress: text("keeper_address").notNull(),
    moveGrantExpiresAt: timestamp("move_grant_expires_at", {
      withTimezone: true
    }),
    stakingGrantExpiresAt: timestamp("staking_grant_expires_at", {
      withTimezone: true
    }),
    feegrantExpiresAt: timestamp("feegrant_expires_at", {
      withTimezone: true
    }),
    moveGrantStatus: grantStatusEnum("move_grant_status")
      .notNull()
      .default("pending"),
    stakingGrantStatus: grantStatusEnum("staking_grant_status")
      .notNull()
      .default("pending"),
    feegrantStatus: grantStatusEnum("feegrant_status")
      .notNull()
      .default("pending"),
    scopeJson: jsonb("scope_json").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true })
      .defaultNow()
      .notNull(),
    updatedAt: timestamp("updated_at", { withTimezone: true })
      .defaultNow()
      .notNull()
  },
  (table) => [
    uniqueIndex("grants_user_id_unique").on(table.userId),
    index("grants_keeper_address_index").on(table.keeperAddress)
  ]
);

export const executions = pgTable(
  "executions",
  {
    id: uuid("id").defaultRandom().primaryKey(),
    strategyId: uuid("strategy_id")
      .notNull()
      .references(() => strategies.id, { onDelete: "cascade" }),
    userId: uuid("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    status: executionStatusEnum("status").notNull().default("queued"),
    inputAmount: text("input_amount").notNull(),
    lpAmount: text("lp_amount"),
    provideTxHash: text("provide_tx_hash"),
    delegateTxHash: text("delegate_tx_hash"),
    errorCode: text("error_code"),
    errorMessage: text("error_message"),
    startedAt: timestamp("started_at", { withTimezone: true })
      .defaultNow()
      .notNull(),
    finishedAt: timestamp("finished_at", { withTimezone: true })
  },
  (table) => [
    index("executions_strategy_id_index").on(table.strategyId),
    index("executions_user_id_index").on(table.userId),
    index("executions_status_index").on(table.status)
  ]
);

export const withdrawals = pgTable(
  "withdrawals",
  {
    id: uuid("id").defaultRandom().primaryKey(),
    userId: uuid("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    strategyId: uuid("strategy_id")
      .notNull()
      .references(() => strategies.id, { onDelete: "cascade" }),
    inputAmount: text("input_amount").notNull(),
    lpAmount: text("lp_amount").notNull(),
    status: withdrawalStatusEnum("status").notNull().default("pending"),
    txHash: text("tx_hash"),
    requestedAt: timestamp("requested_at", { withTimezone: true })
      .defaultNow()
      .notNull(),
    confirmedAt: timestamp("confirmed_at", { withTimezone: true })
  },
  (table) => [
    index("withdrawals_user_id_index").on(table.userId),
    index("withdrawals_strategy_id_index").on(table.strategyId)
  ]
);

export const positions = pgTable(
  "positions",
  {
    id: uuid("id").defaultRandom().primaryKey(),
    strategyId: uuid("strategy_id")
      .notNull()
      .references(() => strategies.id, { onDelete: "cascade" }),
    userId: uuid("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    lastInputBalance: text("last_input_balance").notNull(),
    lastLpBalance: text("last_lp_balance").notNull(),
    lastDelegatedLpBalance: text("last_delegated_lp_balance").notNull(),
    lastRewardSnapshot: text("last_reward_snapshot"),
    lastSyncedAt: timestamp("last_synced_at", { withTimezone: true })
      .defaultNow()
      .notNull()
  },
  (table) => [
    uniqueIndex("positions_strategy_id_unique").on(table.strategyId),
    index("positions_user_id_index").on(table.userId)
  ]
);
