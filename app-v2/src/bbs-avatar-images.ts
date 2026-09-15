export const BBS_AVATAR_MAX_BYTES = 600_000;
const mimeByExt: Readonly<Record<string, string>> = { png: 'image/png', jpg: 'image/jpeg', webp: 'image/webp' };
export const bbsAvatarHash = (value: unknown): value is string => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
export const bbsAvatarExt = (value: unknown): value is 'png' | 'jpg' | 'webp' => typeof value === 'string' && Object.hasOwn(mimeByExt, value);

/** Validate the bounded raster envelope, never an arbitrary URL or file path.
 * SHA validation remains in the backend reader. Roster reads also know size. */
export function bbsAvatarDataUrl(value: unknown, ext: string, expectedBytes?: number): string | null {
  if (!bbsAvatarExt(ext)) return null;
  const prefix = `data:${mimeByExt[ext]};base64,`;
  if (typeof value !== 'string' || value.length > prefix.length + Math.ceil(BBS_AVATAR_MAX_BYTES / 3) * 4
    || !value.startsWith(prefix)) return null;
  const encoded = value.slice(prefix.length);
  if (!encoded.length || encoded.length % 4 !== 0 || !/^[A-Za-z0-9+/]+={0,2}$/.test(encoded)) return null;
  const size = encoded.length / 4 * 3 - (encoded.endsWith('==') ? 2 : encoded.endsWith('=') ? 1 : 0);
  return size > 0 && size <= BBS_AVATAR_MAX_BYTES && (expectedBytes === undefined || size === expectedBytes) ? value : null;
}

type Listener = (src: string | null) => void;
/** Shared queue mechanics, not shared authorization. Each reader supplies its
 * full resource key and a defensive snapshot of exactly its permitted fields.
 * Two reads / 32 distinct jobs / 16 successful LRU values; no timer or retry. */
export function createBbsImageQueue<Ref>(options: {
  read: (ref: Ref) => Promise<string>;
  valid: (ref: Ref) => boolean;
  key: (ref: Ref) => string;
  snapshot: (ref: Ref) => Ref;
}) {
  const cache = new Map<string, string>();
  const jobs = new Map<string, { ref: Ref; listeners: Set<Listener>; started: boolean }>();
  let active = 0;
  function pump() {
    for (const [key, job] of jobs) {
      if (active >= 2) break;
      if (job.started) continue;
      job.started = true;
      active++;
      void Promise.resolve().then(() => job.listeners.size ? options.read(job.ref) : null).then(finish, () => finish(null));
      function finish(src: string | null) {
        active--;
        jobs.delete(key);
        if (src !== null && job.listeners.size > 0) {
          cache.set(key, src);
          while (cache.size > 16) cache.delete(cache.keys().next().value!);
        }
        for (const listener of job.listeners) listener(src);
        job.listeners.clear();
        pump();
      }
    }
  }
  return {
    subscribe(ref: Ref, listener: Listener): () => void {
      if (!options.valid(ref)) { listener(null); return () => {}; }
      const key = options.key(ref);
      const cached = cache.get(key);
      if (cached !== undefined) {
        cache.delete(key); cache.set(key, cached);
        listener(cached);
        return () => {};
      }
      let job = jobs.get(key);
      if (!job) {
        if (jobs.size >= 32) { listener(null); return () => {}; }
        job = { ref: options.snapshot(ref), listeners: new Set(), started: false };
        jobs.set(key, job);
      }
      job.listeners.add(listener);
      pump();
      const subscription = job;
      return () => {
        subscription.listeners.delete(listener);
        if (!subscription.started && subscription.listeners.size === 0 && jobs.get(key) === subscription) jobs.delete(key);
      };
    },
    forget(ref: Ref) { cache.delete(options.key(ref)); },
  };
}
