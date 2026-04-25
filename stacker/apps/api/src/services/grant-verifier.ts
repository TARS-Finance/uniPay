import { RESTClient } from "@initia/initia.js";

export type VerifyGrantBundleInput = {
  granterAddress: string;
  granteeAddress: string;
  moduleAddress: string;
  moduleName: string;
  functionName: string;
  feeAllowedMessage: string;
};

export type VerifyGrantBundleResult = {
  moveGrantActive: boolean;
  feegrantActive: boolean;
};

export interface GrantVerifier {
  verify(input: VerifyGrantBundleInput): Promise<VerifyGrantBundleResult>;
}

type SerializableWithData = {
  toData?: () => unknown;
};

type MoveGrantData = {
  authorization?: {
    "@type"?: string;
    items?: Array<{
      module_address?: string;
      module_name?: string;
      function_names?: string[];
    }>;
  };
};

type AllowedMsgAllowanceData = {
  "@type"?: string;
  allowed_messages?: string[];
};

export class InitiaGrantVerifier implements GrantVerifier {
  constructor(private readonly rest: RESTClient) {}

  async verify(input: VerifyGrantBundleInput): Promise<VerifyGrantBundleResult> {
    const [moveGrants] = await this.rest.authz.grants(
      input.granterAddress,
      input.granteeAddress,
      "/initia.move.v1.MsgExecute"
    );

    return {
      moveGrantActive: moveGrants.some((grant) => {
        const data = toData<MoveGrantData>(grant);

        if (
          data.authorization?.["@type"] !== "/initia.move.v1.ExecuteAuthorization"
          || !Array.isArray(data.authorization.items)
        ) {
          return false;
        }

        return data.authorization.items.some((item) => {
          return (
            item.module_address === input.moduleAddress
            && item.module_name === input.moduleName
            && Array.isArray(item.function_names)
            && item.function_names.includes(input.functionName)
          );
        });
      }),
      feegrantActive: await this.hasAllowedFeeGrant(
        input.granterAddress,
        input.granteeAddress,
        input.feeAllowedMessage
      )
    };
  }

  private async hasAllowedFeeGrant(
    granterAddress: string,
    granteeAddress: string,
    feeAllowedMessage: string
  ) {
    try {
      const allowance = await this.rest.feeGrant.allowance(
        granterAddress,
        granteeAddress
      );
      const data = toData<AllowedMsgAllowanceData>(allowance);

      return (
        data["@type"] === "/cosmos.feegrant.v1beta1.AllowedMsgAllowance"
        && Array.isArray(data.allowed_messages)
        && data.allowed_messages.includes(feeAllowedMessage)
      );
    } catch (error) {
      if (isNotFoundError(error)) {
        return false;
      }

      throw error;
    }
  }
}

function toData<T>(value: unknown): T {
  const serializable = value as SerializableWithData;

  if (
    typeof value === "object"
    && value !== null
    && "toData" in value
    && typeof serializable.toData === "function"
  ) {
    return serializable.toData() as T;
  }

  return value as T;
}

function isNotFoundError(error: unknown) {
  if (typeof error !== "object" || error === null) {
    return false;
  }

  const response = "response" in error ? error.response : null;

  return (
    typeof response === "object"
    && response !== null
    && "status" in response
    && response.status === 404
  );
}
