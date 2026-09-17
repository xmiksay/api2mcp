// Typed fetch wrapper for `/api/*`. Every admin route lives behind a session cookie
// (`server::identity::Caller`'s `FromRequestParts` impl resolves only from that cookie), so a
// 401 here always means "no live session" — the one auth rule this SPA implements is redirecting
// to the server-rendered login page with `next` pointing back at wherever the user was.
//
// The server answers a validation failure with `{"errors": [...]}` (plural — see
// `server::error::ApiError`'s own doc: reporting every problem in one pass is the point) and
// every other failure with `{"error": "..."}`. `ApiError.messages` always holds at least one
// string so a caller that only wants "the" message can use `.message` (the first entry) without
// caring which shape the server used.
export class ApiError extends Error {
  readonly status: number;
  /** Every message the server reported — length 1 except for a 400 validation response. */
  readonly messages: string[];

  constructor(status: number, messages: string[]) {
    super(messages[0] ?? `request failed with status ${status}`);
    this.name = "ApiError";
    this.status = status;
    this.messages = messages.length > 0 ? messages : [this.message];
  }
}

function loginRedirect(): never {
  const next = encodeURIComponent(location.pathname + location.search);
  location.href = `/login?next=${next}`;
  // `location.href` navigates away asynchronously; throwing keeps the current call stack from
  // acting on a response that is about to be replaced by the login page.
  throw new ApiError(401, ["redirecting to /login"]);
}

async function parseBody(res: Response): Promise<unknown> {
  const text = await res.text();
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch {
    // The 401 rejection built by `Caller`'s extractor is plain text, not JSON — see
    // `server::identity`. Anything else non-JSON is treated the same way: no structured body.
    return null;
  }
}

function messagesFrom(body: unknown, fallback: string): string[] {
  if (body && typeof body === "object") {
    const record = body as Record<string, unknown>;
    if (Array.isArray(record.errors)) {
      return record.errors.filter((e): e is string => typeof e === "string");
    }
    if (typeof record.error === "string") return [record.error];
  }
  return [fallback];
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    credentials: "same-origin",
    headers: body !== undefined ? { "Content-Type": "application/json" } : {},
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });

  if (res.status === 401) loginRedirect();
  if (res.status === 204) return undefined as T;

  const payload = await parseBody(res);
  if (!res.ok) {
    throw new ApiError(res.status, messagesFrom(payload, res.statusText || `HTTP ${res.status}`));
  }
  return payload as T;
}

export const api = {
  get: <T>(path: string): Promise<T> => request<T>("GET", path),
  post: <T>(path: string, body?: unknown): Promise<T> => request<T>("POST", path, body),
  put: <T>(path: string, body?: unknown): Promise<T> => request<T>("PUT", path, body),
  delete: <T = void>(path: string): Promise<T> => request<T>("DELETE", path),
};
