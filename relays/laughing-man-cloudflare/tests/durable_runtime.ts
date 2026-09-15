// In-process RPC/storage harness; production uses Cloudflare's actual DurableObject.
export class DurableObject {
  constructor(
    protected ctx: any,
    protected env: any,
  ) {}
}
export class MemoryState {
  data = new Map<string, unknown>();
  alarm: number | null = null;
  private tail: Promise<unknown> = Promise.resolve();
  readonly storage = {
    get: async <T>(key: string): Promise<T | undefined> =>
      structuredClone(this.data.get(key)) as T | undefined,
    put: async (key: string | Record<string, unknown>, value?: unknown) => {
      if (typeof key === 'string') this.data.set(key, structuredClone(value));
      else for (const [k, v] of Object.entries(key)) this.data.set(k, structuredClone(v));
    },
    delete: async (key: string) => this.data.delete(key),
    deleteAll: async () => {
      this.data.clear();
    },
    list: async <T>({ prefix }: { prefix: string }): Promise<Map<string, T>> =>
      new Map(
        [...this.data]
          .filter(([key]) => key.startsWith(prefix))
          .map(([k, v]) => [k, structuredClone(v) as T]),
      ),
    setAlarm: async (at: number) => {
      this.alarm = at;
    },
    deleteAlarm: async () => {
      this.alarm = null;
    },
    transaction: async <T>(fn: (storage: MemoryState['storage']) => Promise<T>): Promise<T> =>
      this.blockConcurrencyWhile(async () => {
        const before = structuredClone(this.data);
        try {
          return await fn(this.storage);
        } catch (error) {
          this.data = before;
          throw error;
        }
      }),
  };
  async blockConcurrencyWhile<T>(fn: () => Promise<T>): Promise<T> {
    const run = this.tail.then(fn);
    this.tail = run.catch(() => {});
    return run;
  }
}
