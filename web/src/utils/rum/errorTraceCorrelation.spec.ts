import { describe, expect, it } from "vitest";
import { errorSpanId, errorTraceId } from "./errorTraceCorrelation";

const timestamp = 1_700_000_000_000_000;
const trace = "00000000000000000000000000000002";
const otherTrace = "00000000000000000000000000000001";

function resource(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    type: "resource",
    resource_type: "xhr",
    resource_method: "POST",
    resource_url: "https://example.com/api/items/query",
    resource_status_code: 500,
    _timestamp: timestamp + 100,
    _o2_trace_id: trace,
    ...overrides,
  };
}

function error(
  events: Record<string, unknown>[],
  overrides: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    _timestamp: timestamp,
    context_url: "https://example.com/api/items/query",
    context_method: "POST",
    context_responsestatus: "500",
    events,
    ...overrides,
  };
}

describe("errorTraceId", () => {
  it("prefers and normalizes an explicit trace over breadcrumb candidates", () => {
    expect(errorTraceId(error([resource()], { _oo_trace_id: "1" }))).toBe(otherTrace);
  });

  it("uses the matching failure instead of the first successful request", () => {
    expect(
      errorTraceId(
        error([
          resource({ resource_method: "GET", resource_status_code: 200, _o2_trace_id: otherTrace }),
          resource(),
        ]),
      ),
    ).toBe(trace);
  });

  it.each([
    { resource_method: "GET" },
    { resource_status_code: 200 },
    { resource_url: "https://example.com/api/other" },
    { resource_url: "https://other.example.com/api/items/query" },
    { resource_type: "image" },
    { _timestamp: timestamp - 30_000_000 },
  ])("does not correlate a different request: %j", (overrides) => {
    expect(errorTraceId(error([resource(overrides)]))).toBe("");
  });

  it("does not infer a request for a JavaScript error without request metadata", () => {
    expect(errorTraceId({ _timestamp: timestamp, events: [resource()] })).toBe("");
  });

  it("requires a method rather than guessing between requests to the same URL", () => {
    expect(errorTraceId(error([resource()], { context_method: undefined }))).toBe("");
  });

  it("leaves repeated matching requests ambiguous even when one is nearer", () => {
    expect(
      errorTraceId(
        error([resource(), resource({ _timestamp: timestamp - 200, _o2_trace_id: otherTrace })]),
      ),
    ).toBe("");
  });

  it("does not choose a traced request when another matching candidate lacks its trace", () => {
    expect(errorTraceId(error([resource(), resource({ _o2_trace_id: undefined })]))).toBe("");
  });

  it("accepts duplicate breadcrumbs of the same request trace", () => {
    expect(errorTraceId(error([resource(), resource()]))).toBe(trace);
  });

  it("matches standard error resource fields without application-specific context", () => {
    expect(
      errorTraceId({
        _timestamp: timestamp,
        error_resource_url: "https://example.com/api/items/query",
        error_resource_method: "POST",
        error_resource_status_code: 500,
        events: [resource({ resource_type: "fetch", _o2_trace_id: undefined, _oo_trace_id: "2" })],
      }),
    ).toBe(trace);
  });

  it("handles a nested resource and a sanitized relative error URL", () => {
    expect(
      errorTraceId(
        error(
          [
            {
              type: "resource",
              _timestamp: timestamp,
              _oo_trace_id: "2",
              resource: {
                type: "fetch",
                method: "POST",
                status_code: 500,
                url: "https://example.com/api/items/query?page=2#section",
              },
            },
          ],
          { context_url: "/api/items/query" },
        ),
      ),
    ).toBe(trace);
  });

  it("uses the completion of a long request when its resource timestamp is the start", () => {
    expect(
      errorTraceId(
        error([
          resource({ _timestamp: timestamp - 10_000_000, resource_duration: 10_000_000_000 }),
        ]),
      ),
    ).toBe(trace);
  });

  it("uses query parameters when the error URL retains them", () => {
    expect(
      errorTraceId(
        error([resource({ resource_url: "https://example.com/api/items/query?page=2" })], {
          context_url: "https://example.com/api/items/query?page=1",
        }),
      ),
    ).toBe("");
  });

  it("does not let an empty optional status hide the available response status", () => {
    expect(
      errorTraceId(error([resource({ resource_status_code: 200 })], { context_http_status: "" })),
    ).toBe("");
  });
});

const parentSpan = "79b0553002e94471";
const traceparent = `00-${trace}-${parentSpan}-01`;

describe("application trace context", () => {
  it.each([
    { _o2: { trace_id: trace } },
    { _oo: { trace_id: trace } },
    { context_trace_id: trace },
    { context_traceid: trace },
    { context: { trace_id: trace } },
    { context: { traceId: trace } },
    { context_traceparent: traceparent },
    { context: { traceparent } },
  ])("reads historical and current context spellings: %j", (context) => {
    expect(errorTraceId(context)).toBe(trace);
  });

  it("uses explicit application context before a different resource breadcrumb", () => {
    expect(
      errorTraceId(
        error([resource({ _o2_trace_id: otherTrace })], {
          context_trace_id: trace,
        }),
      ),
    ).toBe(trace);
  });

  it.each([
    { context_trace_id: "00000000000000000000000000000000" },
    { context_trace_id: "not-a-trace" },
    { context_traceparent: `00-${trace}-0000000000000000-01` },
    { context_traceparent: `00-${"0".repeat(32)}-${parentSpan}-01` },
    { context_traceparent: `ff-${trace}-${parentSpan}-01` },
    { context_traceparent: `00-${trace}-${parentSpan}-01-extra` },
  ])("does not expose invalid application trace evidence: %j", (context) => {
    expect(errorTraceId(context)).toBe("");
    expect(errorSpanId(context)).toBe("");
  });

  it.each([
    { _o2_trace_id: trace, _o2_span_id: parentSpan },
    { _oo_trace_id: trace, _oo_span_id: parentSpan },
    { _o2: { trace_id: trace, span_id: parentSpan } },
    { _oo: { trace_id: trace, span_id: parentSpan } },
    { context_trace_id: trace, context_parent_span_id: parentSpan },
    { context_traceid: trace, context_parentspanid: parentSpan },
    { context: { trace_id: trace, parent_span_id: parentSpan } },
    { context: { traceId: trace, parentSpanId: parentSpan } },
    { context_traceparent: traceparent },
    { context: { traceparent } },
  ])("extracts the propagated browser parent with its trace: %j", (context) => {
    expect(errorSpanId(context)).toBe(parentSpan);
  });

  it("prefers the SDK span paired with the selected SDK trace over application context", () => {
    expect(
      errorSpanId({
        _o2_trace_id: trace,
        _o2_span_id: parentSpan,
        context_trace_id: otherTrace,
        context_parent_span_id: "1111111111111111",
      }),
    ).toBe(parentSpan);
  });

  it.each([
    { context_parent_span_id: parentSpan },
    { context_trace_id: trace, context_parent_span_id: "0000000000000000" },
    { context_trace_id: trace, context_parent_span_id: "invalid-span" },
    { _o2_trace_id: otherTrace, context_trace_id: trace, context_parent_span_id: parentSpan },
    { context_trace_id: trace, context_traceparent: `00-${otherTrace}-${parentSpan}-01` },
  ])("does not attach a parent from unassociated or invalid context: %j", (context) => {
    expect(errorSpanId(context)).toBe("");
  });
});
