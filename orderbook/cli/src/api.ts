import {
  ApiAcceptQuoteRequest,
  ApiAcceptQuoteResponse,
  ApiCreateRfqRequest,
  ApiCreateRfqResponse,
  ApiCreateOrderRequest,
  ApiEnvelope,
  ApiMatchedOrderResponse,
  ApiQuoteRequest,
  ApiQuoteResponse,
  ApiTradeStatus,
} from "./types";

export class ApiError extends Error {
  constructor(
    message: string,
    public status: number,
    public detail?: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export class MungerApi {
  private readonly baseUrl: string;

  constructor(baseUrl: string) {
    this.baseUrl = baseUrl.endsWith("/") ? baseUrl.slice(0, -1) : baseUrl;
  }

  async createRfq(payload: ApiCreateRfqRequest): Promise<ApiCreateRfqResponse> {
    return this.requestLegacy<ApiCreateRfqResponse>("/rfq", {
      method: "POST",
      body: JSON.stringify(payload),
    });
  }

  async acceptQuote(payload: ApiAcceptQuoteRequest): Promise<ApiAcceptQuoteResponse> {
    return this.requestLegacy<ApiAcceptQuoteResponse>("/trade", {
      method: "POST",
      body: JSON.stringify(payload),
    });
  }

  async quote(params: ApiQuoteRequest): Promise<ApiQuoteResponse> {
    const searchParams = new URLSearchParams();
    searchParams.set("from", params.from);
    searchParams.set("to", params.to);

    if (params.from_amount) {
      searchParams.set("from_amount", params.from_amount);
    }

    if (params.to_amount) {
      searchParams.set("to_amount", params.to_amount);
    }

    if (params.strategy_id) {
      searchParams.set("strategy_id", params.strategy_id);
    }

    if (params.slippage !== undefined) {
      searchParams.set("slippage", String(params.slippage));
    }

    if (params.affiliate_fee !== undefined) {
      searchParams.set("affiliate_fee", String(params.affiliate_fee));
    }

    return this.requestEnvelope<ApiQuoteResponse>(`/quote?${searchParams.toString()}`, {
      method: "GET",
    });
  }

  async createOrder(payload: ApiCreateOrderRequest): Promise<ApiMatchedOrderResponse> {
    return this.requestEnvelope<ApiMatchedOrderResponse>("/orders", {
      method: "POST",
      body: JSON.stringify(payload),
    });
  }

  async getOrder(orderId: string): Promise<ApiMatchedOrderResponse> {
    return this.requestEnvelope<ApiMatchedOrderResponse>(`/orders/${encodeURIComponent(orderId)}`, {
      method: "GET",
    });
  }

  async getTradeStatus(tradeId: string): Promise<ApiTradeStatus> {
    return this.requestLegacy<ApiTradeStatus>(`/trade/${tradeId}`, {
      method: "GET",
    });
  }

  private async requestEnvelope<T>(path: string, init: RequestInit): Promise<T> {
    const response = await this.requestDirect<ApiEnvelope<T>>(path, init);
    if (!response.ok) {
      throw new Error("API response was marked as failed");
    }
    return response.data;
  }

  private async requestLegacy<T>(path: string, init: RequestInit): Promise<T> {
    return this.requestWithPrefix<T>("/v1", path, init);
  }

  private async requestDirect<T>(path: string, init: RequestInit): Promise<T> {
    return this.requestWithPrefix<T>("", path, init);
  }

  private async requestWithPrefix<T>(
    prefix: string,
    path: string,
    init: RequestInit,
  ): Promise<T> {
    const response = await fetch(`${this.baseUrl}${prefix}${path}`, {
      ...init,
      headers: {
        ...(init.body ? { "Content-Type": "application/json" } : {}),
        ...(init.headers ?? {}),
      },
    });

    if (!response.ok) {
      const detail = await response
        .text()
        .catch(() => "Unable to read request error body");
      throw new ApiError(
        `HTTP ${response.status} for ${path}`,
        response.status,
        detail,
      );
    }

    const body = (await response.json()) as T;
    return body;
  }
}
