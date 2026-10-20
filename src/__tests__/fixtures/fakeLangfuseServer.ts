import http from "node:http";
import type { AddressInfo } from "node:net";
import type { LangfuseObservation } from "../../adapters/langfuseObservationLister.js";

/** One request the fake received on `GET /api/public/v2/observations`. */
export interface RecordedRequest {
  path: string;
  query: Record<string, string>;
  authorized: boolean;
}

export interface FakeLangfuseServer {
  url: string;
  requests: RecordedRequest[];
  close(): Promise<void>;
}

export const TEST_PUBLIC_KEY = "test-public-key";
export const TEST_SECRET_KEY = "test-secret-key";
const EXPECTED_AUTH = `Basic ${Buffer.from(`${TEST_PUBLIC_KEY}:${TEST_SECRET_KEY}`).toString("base64")}`;

/**
 * A loopback stand-in for the Langfuse v2 observations endpoint (never the
 * real API): serves `observations` in pages of `pageSize` behind an opaque
 * cursor, filtered as the v2 endpoint does (`traceId` and `sessionId` exact,
 * `fromStartTime` on `startTime`), or answers every request with `failWith` status. The last
 * page's `meta` is `lastPageMeta` (Langfuse Cloud answers `{}`). Past
 * `maxRequests` requests it answers 418, so a client that never stops fails
 * fast instead of looping.
 */
export async function startFakeLangfuse(options: {
  observations?: LangfuseObservation[];
  pageSize?: number;
  failWith?: number;
  lastPageMeta?: Record<string, unknown>;
  maxRequests?: number;
}): Promise<FakeLangfuseServer> {
  const { observations = [], pageSize = 1000, failWith, lastPageMeta = {}, maxRequests = 100 } = options;
  const requests: RecordedRequest[] = [];
  const server = http.createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    requests.push({
      path: url.pathname,
      query: Object.fromEntries(url.searchParams),
      authorized: req.headers.authorization === EXPECTED_AUTH,
    });
    res.setHeader("content-type", "application/json");
    const status = failWith ?? (requests.length > maxRequests ? 418 : undefined);
    if (status !== undefined) {
      res.statusCode = status;
      res.end(JSON.stringify({ message: "Invalid credentials" }));
      return;
    }
    const traceId = url.searchParams.get("traceId");
    const sessionId = url.searchParams.get("sessionId");
    const fromStartTime = url.searchParams.get("fromStartTime");
    const matching = observations.filter(
      (o) =>
        (traceId === null || o.traceId === traceId) &&
        (sessionId === null || o.sessionId === sessionId) &&
        (fromStartTime === null || o.startTime >= fromStartTime)
    );
    const offset = Number(url.searchParams.get("cursor")?.replace("page-", "") ?? 0);
    const next = offset + pageSize;
    res.end(
      JSON.stringify({
        data: matching.slice(offset, next),
        meta: next < matching.length ? { cursor: `page-${next}` } : lastPageMeta,
      })
    );
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  return {
    url: `http://127.0.0.1:${port}`,
    requests,
    close: () => new Promise((resolve) => server.close(() => resolve())),
  };
}
