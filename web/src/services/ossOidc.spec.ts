// Copyright 2026 OpenObserve Inc.
import { beforeEach, describe, expect, it, vi } from "vitest";

beforeEach(() => {
  vi.resetModules();
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

describe("OSS OIDC browser session lifecycle", () => {
  it("only establishes SSO browser state from a successful backend bootstrap", async () => {
    const { bootstrapOidc, ossOidc, oidcActive } = await import("./ossOidc");
    vi.stubGlobal("localStorage", { getItem: () => "untrusted-profile" });
    vi.stubGlobal(
      "fetch",
      async () =>
        new Response(
          JSON.stringify({ enabled: true, authenticated: false, csrf: "browser-proof" }),
          { status: 200 },
        ),
    );
    await bootstrapOidc();
    expect(oidcActive()).toBe(false);
    expect(ossOidc.authenticated).toBe(false);
    vi.stubGlobal("fetch", async () => new Response("unavailable", { status: 503 }));
    await expect(bootstrapOidc()).rejects.toThrow("Session unavailable");
    expect(oidcActive()).toBe(false);
  });
  it("requires successful server revocation before clearing the SSO account marker", async () => {
    const { bootstrapOidc, logoutOidc, oidcActive } = await import("./ossOidc");
    let revoked = false;
    vi.stubGlobal("fetch", async (input: string, init?: RequestInit) => {
      if (input.endsWith("/bootstrap"))
        return new Response(
          JSON.stringify({
            enabled: true,
            authenticated: true,
            csrf: "session-proof",
            user: {
              email: "local@example.invalid",
              first_name: "",
              last_name: "",
              organizations: ["allowed"],
            },
          }),
          { status: 200 },
        );
      const headers = new Headers(init?.headers);
      if (init?.method !== "POST" || headers.get("x-o2-csrf") !== "session-proof")
        return new Response(null, { status: 403 });
      return new Response(null, { status: revoked ? 204 : 503 });
    });
    await bootstrapOidc();
    await expect(logoutOidc()).rejects.toThrow("Logout unavailable");
    expect(oidcActive()).toBe(true);
    revoked = true;
    await logoutOidc();
    expect(oidcActive()).toBe(false);
  });
  it("applies cookie CSRF and removes stale Authorization on raw same-origin API fetches", async () => {
    const requests: Request[] = [];
    vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
      requests.push(input instanceof Request ? input : new Request(input, init));
      return new Response("{}", { status: 200 });
    });
    const { ossOidc, installOidcFetch } = await import("./ossOidc");
    ossOidc.authenticated = true;
    ossOidc.csrf = "session-proof";
    installOidcFetch();
    await fetch(`${window.location.origin}/api/allowed/_search`, {
      method: "POST",
      headers: { Authorization: "Basic stale-user" },
      body: "query",
    });
    expect(requests[0].headers.has("Authorization")).toBe(false);
    expect(requests[0].headers.get("x-o2-csrf")).toBe("session-proof");
    expect(await requests[0].text()).toBe("query");
    await fetch("https://external.example/api/data", {
      headers: { Authorization: "Bearer external-credential" },
    });
    expect(requests[1].headers.get("Authorization")).toBe("Bearer external-credential");
    expect(requests[1].headers.has("x-o2-csrf")).toBe(false);
  });
});
