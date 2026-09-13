export type EditorOperation = "analysis" | "render" | "update";

export interface OperationToken {
  kind: EditorOperation;
  sequence: number;
}

/** Synchronous admission also covers two native events before React rerenders. */
export class EditorOperationGate {
  private active: OperationToken | null = null;
  private sequence = 0;
  private closing = false;

  get blocked() { return this.closing || this.active !== null; }
  get updating() { return this.active?.kind === "update"; }

  begin(kind: EditorOperation): OperationToken | null {
    if (this.blocked) return null;
    const token = { kind, sequence: ++this.sequence };
    this.active = token;
    return token;
  }

  current(token: OperationToken) {
    return !this.closing && this.active === token;
  }

  finish(token: OperationToken) {
    if (this.active === token) this.active = null;
  }

  setClosing(value: boolean) { this.closing = value; }
}

/** Writes every accepted immutable snapshot in order; a failure remains retryable. */
export class DraftWriter<Request, Result> {
  private tail: Promise<unknown> = Promise.resolve();
  private saved: { key: string; result: Result } | null = null;
  private write: (request: Request) => Promise<Result>;

  constructor(write: (request: Request) => Promise<Result>) { this.write = write; }

  invalidateSavedSnapshot() { this.saved = null; }

  persist(request: Request): Promise<Result> {
    const key = JSON.stringify(request);
    const snapshot = JSON.parse(key) as Request;
    const pending = this.tail.catch(() => undefined).then(async () => {
      if (this.saved?.key === key) return this.saved.result;
      const result = await this.write(snapshot);
      this.saved = { key, result };
      return result;
    });
    this.tail = pending;
    return pending;
  }

  async flush(request: Request | null) {
    if (request !== null) return this.persist(request);
    await this.tail;
    return null;
  }
}
