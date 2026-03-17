// ── Event bus abstraction ──────────────────────────────────────────────────────
//
// In development (Vite proxy → Rust server on :3000):
//   ws://localhost:3000/ws
// In production (Rust server serves the built React app):
//   ws://<same-host>/ws

type UnlistenFn = () => void;
type Handler<T> = (payload: T) => void;

const WS_URL = `${location.protocol === "https:" ? "wss:" : "ws:"}//${
  import.meta.env.DEV ? "localhost:3000" : location.host
}/ws`;

// ── WebSocket bus ─────────────────────────────────────────────────────────────

class EventBus {
  private ws: WebSocket | null = null;
  private handlers = new Map<string, Set<Handler<unknown>>>();
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private dead = false;

  constructor() {
    this.connect();
  }

  private connect() {
    if (this.dead) return;
    this.ws = new WebSocket(WS_URL);

    this.ws.onmessage = (msg: MessageEvent<string>) => {
      try {
        const { event, payload } = JSON.parse(msg.data) as {
          event: string;
          payload: unknown;
        };
        this.handlers.get(event)?.forEach((h) => h(payload));
      } catch {
        // ignore malformed frames
      }
    };

    this.ws.onclose = () => {
      if (this.dead) return;
      this.reconnectTimer = setTimeout(() => this.connect(), 2000);
    };

    this.ws.onerror = () => {
      this.ws?.close();
    };
  }

  /** Subscribe to a named event. Returns an unlisten function. */
  listen<T>(event: string, handler: Handler<T>): UnlistenFn {
    if (!this.handlers.has(event)) this.handlers.set(event, new Set());
    this.handlers.get(event)!.add(handler as Handler<unknown>);
    return () => this.handlers.get(event)?.delete(handler as Handler<unknown>);
  }

  /** Send a command to the server over the same WebSocket. */
  send(type: string, payload?: unknown) {
    const msg = JSON.stringify({ type, ...(payload ? { payload } : {}) });
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(msg);
    }
  }

  destroy() {
    this.dead = true;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.ws?.close();
  }
}

export const eventBus = new EventBus();
