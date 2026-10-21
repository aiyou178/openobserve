// Copyright 2026 OpenObserve Inc.
import { reactive } from "vue";

interface User {
  email: string;
  first_name: string;
  last_name: string;
  organizations: string[];
}
interface Bootstrap {
  enabled: boolean;
  authenticated: boolean;
  session_invalid?: boolean;
  csrf?: string;
  user?: User;
}
export const ossOidc = reactive<Bootstrap>({ enabled: false, authenticated: false });
const webIndex = window.location.pathname.search(/\/web(?:\/|$)/);
const prefix = webIndex < 0 ? "" : window.location.pathname.slice(0, webIndex);
const endpoint = `${window.location.origin}${prefix}/auth/oidc`;

export async function bootstrapOidc(): Promise<Bootstrap> {
  const response = await fetch(`${endpoint}/bootstrap`, {
    credentials: "same-origin",
    cache: "no-store",
    headers: { "x-o2-bootstrap": "1" },
  });
  if (response.status === 404)
    return Object.assign(ossOidc, { enabled: false, authenticated: false });
  if (!response.ok) throw new Error("Session unavailable");
  const result: Bootstrap = await response.json();
  Object.assign(ossOidc, { user: undefined, csrf: undefined, session_invalid: false }, result);
  if (result.authenticated) sessionStorage.setItem("o2-auth-mode", "oidc");
  return result;
}

export async function loginOidc(): Promise<void> {
  await bootstrapOidc();
  const response = await fetch(`${endpoint}/login`, {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json", "x-o2-csrf": ossOidc.csrf ?? "" },
    body: JSON.stringify({ return_path: `${prefix}/web/` }),
  });
  if (!response.ok) throw new Error("Login unavailable");
  const result: { authorize_url: string } = await response.json();
  window.location.assign(result.authorize_url);
}

export async function logoutOidc(): Promise<void> {
  await bootstrapOidc();
  const response = await fetch(`${endpoint}/logout`, {
    method: "POST",
    credentials: "same-origin",
    headers: { "x-o2-csrf": ossOidc.csrf ?? "" },
  });
  if (!response.ok) throw new Error("Logout unavailable");
  ossOidc.authenticated = false;
  sessionStorage.removeItem("o2-auth-mode");
}

export function oidcActive(): boolean {
  return ossOidc.authenticated || sessionStorage.getItem("o2-auth-mode") === "oidc";
}

export function oidcLoginPath(): string {
  return `${prefix}/web/login`;
}

export const sourceUrl = new URL(
  import.meta.env.VITE_CORRESPONDING_SOURCE_URL || "https://github.com/aiyou178/openobserve",
  document.baseURI,
).toString();

export function installOidcFetch(): void {
  const original = globalThis.fetch.bind(globalThis);
  globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = new URL(input instanceof Request ? input.url : String(input), window.location.href);
    if (
      !oidcActive() ||
      url.origin !== window.location.origin ||
      !url.pathname.startsWith(`${prefix}/api/`)
    )
      return original(input, init);
    const request = new Request(input, init);
    request.headers.delete("Authorization");
    request.headers.set("x-o2-csrf", ossOidc.csrf ?? "");
    const response = await original(request);
    if (response.status === 401) {
      await logoutOidc().finally(() => window.location.assign(oidcLoginPath()));
    }
    return response;
  };
}
