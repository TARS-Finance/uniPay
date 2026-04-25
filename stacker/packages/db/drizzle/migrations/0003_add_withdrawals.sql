CREATE TYPE "public"."withdrawal_status" AS ENUM('pending', 'confirmed', 'failed');
--> statement-breakpoint
CREATE TABLE "withdrawals" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid NOT NULL,
	"strategy_id" uuid NOT NULL,
	"input_amount" text NOT NULL,
	"lp_amount" text NOT NULL,
	"status" "withdrawal_status" DEFAULT 'pending' NOT NULL,
	"tx_hash" text,
	"requested_at" timestamp with time zone DEFAULT now() NOT NULL,
	"confirmed_at" timestamp with time zone
);
--> statement-breakpoint
ALTER TABLE "withdrawals" ADD CONSTRAINT "withdrawals_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "withdrawals" ADD CONSTRAINT "withdrawals_strategy_id_strategies_id_fk" FOREIGN KEY ("strategy_id") REFERENCES "public"."strategies"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
CREATE INDEX "withdrawals_user_id_index" ON "withdrawals" USING btree ("user_id");
--> statement-breakpoint
CREATE INDEX "withdrawals_strategy_id_index" ON "withdrawals" USING btree ("strategy_id");
