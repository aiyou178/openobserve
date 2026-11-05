// Copyright 2026 OpenObserve Inc.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

import { normalizeTraceId } from "./fields";

type RumEvent = Record<string, unknown>;

const REQUEST_TIME_TOLERANCE_US = 1_000_000;

function isRecord(value: unknown): value is RumEvent {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function firstString(...values: unknown[]): string {
  return (
    values
      .find((value): value is string => typeof value === "string" && value.trim() !== "")
      ?.trim() ?? ""
  );
}

function numeric(value: unknown): number | undefined {
  if (typeof value !== "number" && (typeof value !== "string" || value.trim() === "")) return;
  const result = Number(value);
  return Number.isFinite(result) ? result : undefined;
}

function resourceField(event: RumEvent, name: string): unknown {
  return event[`resource_${name}`] ?? (isRecord(event.resource) ? event.resource[name] : undefined);
}

function requestMatches(left: string, right: string): boolean {
  try {
    const expected = new URL(left, "http://rum.invalid");
    const actual = new URL(right, "http://rum.invalid");
    const sameOrigin =
      expected.origin === "http://rum.invalid" || expected.origin === actual.origin;
    return (
      sameOrigin &&
      expected.pathname === actual.pathname &&
      (!expected.search || expected.search === actual.search)
    );
  } catch {
    return false;
  }
}

function validTraceId(value: unknown): string {
  if (typeof value !== "string") return "";
  const traceId = normalizeTraceId(value);
  return /^0+$/.test(traceId) ? "" : traceId;
}

function validSpanId(value: unknown): string {
  if (typeof value !== "string") return "";
  const spanId = value.trim().toLowerCase();
  return /^[0-9a-f]{16}$/.test(spanId) && !/^0+$/.test(spanId) ? spanId : "";
}

function traceContexts(event: RumEvent): { traceId: string; spanId: string }[] {
  const context = isRecord(event.context) ? event.context : {};
  const o2 = isRecord(event._o2) ? event._o2 : {};
  const oo = isRecord(event._oo) ? event._oo : {};
  const pairs = [
    [event._o2_trace_id, event._o2_span_id],
    [event._oo_trace_id, event._oo_span_id],
    [o2.trace_id, o2.span_id],
    [oo.trace_id, oo.span_id],
    [event.context_trace_id, event.context_parent_span_id],
    [event.context_traceid, event.context_parentspanid],
    [context.trace_id, context.parent_span_id],
    [context.traceId, context.parentSpanId],
  ];
  const contexts = pairs.map(([traceId, spanId]) => ({
    traceId: validTraceId(traceId),
    spanId: validSpanId(spanId),
  }));
  for (const value of [event.context_traceparent, context.traceparent]) {
    if (typeof value !== "string") continue;
    const match = /^00-([0-9a-f]{32})-([0-9a-f]{16})-[0-9a-f]{2}$/i.exec(value.trim());
    if (!match) continue;
    const traceId = validTraceId(match[1]);
    const spanId = validSpanId(match[2]);
    if (traceId && spanId) contexts.push({ traceId, spanId });
  }
  return contexts.filter(({ traceId }) => traceId !== "");
}

export function errorSpanId(error: RumEvent): string {
  const traceId = errorTraceId(error);
  if (!traceId) return "";
  return (
    traceContexts(error).find((context) => context.traceId === traceId && context.spanId)?.spanId ??
    ""
  );
}

export function errorTraceId(error: RumEvent): string {
  const directTrace = traceContexts(error)[0]?.traceId;
  if (directTrace) return directTrace;

  const errorResource =
    isRecord(error.error) && isRecord(error.error.resource) ? error.error.resource : {};
  const context = isRecord(error.context) ? error.context : {};
  const url = firstString(
    error.error_resource_url,
    errorResource.url,
    error.context_request_url,
    error.context_url,
    context.url,
  );
  const method = firstString(
    error.error_resource_method,
    errorResource.method,
    error.context_request_method,
    error.context_method,
    context.method,
  ).toUpperCase();
  const status = [
    error.error_resource_status_code,
    errorResource.status_code,
    error.context_http_status,
    error.context_responsestatus,
  ]
    .map(numeric)
    .find((value) => value !== undefined);
  const timestamp = numeric(error._timestamp);
  if (!url || !method || timestamp === undefined || !Array.isArray(error.events)) return "";

  const traces = new Set<string>();
  for (const event of error.events) {
    if (!isRecord(event) || event.type !== "resource") continue;
    if (!["xhr", "fetch"].includes(firstString(resourceField(event, "type")))) continue;
    if (firstString(resourceField(event, "method")).toUpperCase() !== method) continue;
    if (!requestMatches(url, firstString(resourceField(event, "url")))) continue;
    if (status !== undefined && numeric(resourceField(event, "status_code")) !== status) continue;

    const resourceTime = numeric(event._timestamp);
    if (resourceTime === undefined) continue;
    const durationUs = Math.max(0, numeric(resourceField(event, "duration")) ?? 0) / 1000;
    const distance = Math.min(
      Math.abs(timestamp - resourceTime),
      Math.abs(timestamp - resourceTime - durationUs),
    );
    if (distance > REQUEST_TIME_TOLERANCE_US) continue;
    const trace = traceContexts(event)[0]?.traceId;
    if (!trace) return "";
    traces.add(trace);
  }

  // A session can contain concurrent requests to the same endpoint; ambiguous evidence must not select a trace.
  return traces.size === 1 ? (traces.values().next().value ?? "") : "";
}
