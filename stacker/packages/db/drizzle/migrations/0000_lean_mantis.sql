CREATE TYPE "public"."execution_status" AS ENUM('queued', 'providing', 'delegating', 'success', 'failed', 'retryable');--> statement-breakpoint
CREATE TYPE "public"."grant_status" AS ENUM('pending', 'active', 'revoked', 'expired');--> statement-breakpoint
CREATE TYPE "public"."input_denom" AS ENUM('usdc', 'iusdc');--> statement-breakpoint
CREATE TYPE "public"."strategy_status" AS ENUM('draft', 'grant_pending', 'active', 'executing', 'partial_lp', 'paused', 'expired', 'error');--> statement-breakpoint
CREATE TABLE "executions" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"strategy_id" uuid NOT NULL,
	"user_id" uuid NOT NULL,
	"status" "execution_status" DEFAULT 'queued' NOT NULL,
	"input_amount" text NOT NULL,
	"lp_amount" text,
	"provide_tx_hash" text,
	"delegate_tx_hash" text,
	"error_code" text,
	"error_message" text,
	"started_at" timestamp with time zone DEFAULT now() NOT NULL,
	"finished_at" timestamp with time zone
);
--> statement-breakpoint
CREATE TABLE "grants" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid NOT NULL,
	"keeper_address" text NOT NULL,
	"move_grant_expires_at" timestamp with time zone,
	"staking_grant_expires_at" timestamp with time zone,
	"feegrant_expires_at" timestamp with time zone,
	"move_grant_status" "grant_status" DEFAULT 'pending' NOT NULL,
	"staking_grant_status" "grant_status" DEFAULT 'pending' NOT NULL,
	"feegrant_status" "grant_status" DEFAULT 'pending' NOT NULL,
	"scope_json" jsonb NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "positions" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"strategy_id" uuid NOT NULL,
	"user_id" uuid NOT NULL,
	"last_input_balance" text NOT NULL,
	"last_lp_balance" text NOT NULL,
	"last_delegated_lp_balance" text NOT NULL,
	"last_reward_snapshot" text,
	"last_synced_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "strategies" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid NOT NULL,
	"status" "strategy_status" DEFAULT 'draft' NOT NULL,
	"input_denom" "input_denom" NOT NULL,
	"target_pool_id" text NOT NULL,
	"dex_module_address" text NOT NULL,
	"dex_module_name" text NOT NULL,
	"validator_address" text NOT NULL,
	"min_balance_amount" text NOT NULL,
	"max_amount_per_run" text NOT NULL,
	"max_slippage_bps" text NOT NULL,
	"cooldown_seconds" text NOT NULL,
	"last_executed_at" timestamp with time zone,
	"next_eligible_at" timestamp with time zone,
	"pause_reason" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "users" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"initia_address" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
ALTER TABLE "executions" ADD CONSTRAINT "executions_strategy_id_strategies_id_fk" FOREIGN KEY ("strategy_id") REFERENCES "public"."strategies"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "executions" ADD CONSTRAINT "executions_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "grants" ADD CONSTRAINT "grants_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "positions" ADD CONSTRAINT "positions_strategy_id_strategies_id_fk" FOREIGN KEY ("strategy_id") REFERENCES "public"."strategies"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "positions" ADD CONSTRAINT "positions_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "strategies" ADD CONSTRAINT "strategies_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "executions_strategy_id_index" ON "executions" USING btree ("strategy_id");--> statement-breakpoint
CREATE INDEX "executions_user_id_index" ON "executions" USING btree ("user_id");--> statement-breakpoint
CREATE INDEX "executions_status_index" ON "executions" USING btree ("status");--> statement-breakpoint
CREATE UNIQUE INDEX "grants_user_id_unique" ON "grants" USING btree ("user_id");--> statement-breakpoint
CREATE INDEX "grants_keeper_address_index" ON "grants" USING btree ("keeper_address");--> statement-breakpoint
CREATE UNIQUE INDEX "positions_strategy_id_unique" ON "positions" USING btree ("strategy_id");--> statement-breakpoint
CREATE INDEX "positions_user_id_index" ON "positions" USING btree ("user_id");--> statement-breakpoint
CREATE INDEX "strategies_user_id_index" ON "strategies" USING btree ("user_id");--> statement-breakpoint
CREATE INDEX "strategies_status_index" ON "strategies" USING btree ("status");--> statement-breakpoint
CREATE UNIQUE INDEX "users_initia_address_unique" ON "users" USING btree ("initia_address");