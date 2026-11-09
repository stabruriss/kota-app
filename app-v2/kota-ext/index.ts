import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
import { registerInjection } from './bridge.ts';
import { registerTools } from './tools.ts';
import { registerAuth } from './auth.ts';
import { registerAutoRoute } from './auto-route.ts';
import { registerJevVerify } from './jev.ts';
import { registerLocalCapabilities } from './capabilities.ts';
import { registerShadowClones } from './shadow-clone.ts';
import { registerHistory } from './history.ts';

/** Explicitly loaded by Kota Local; never installed into a user's normal Pi configuration. */
export default async function kotaExtension(pi: ExtensionAPI) {
  registerInjection(pi);
  registerTools(pi);
  registerAuth(pi);
  registerAutoRoute(pi);
  registerJevVerify(pi);
  registerShadowClones(pi);
  registerHistory(pi);
  await registerLocalCapabilities(pi);
}
