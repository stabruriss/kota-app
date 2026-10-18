/** TEST ONLY: deterministic, side-effect-free tool for the native Magpie transport test. */
import { Type } from '@sinclair/typebox';
import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
export default function (pi: ExtensionAPI) {
  pi.registerTool({ name: 'fixture_echo', label: 'Fixture echo', description: 'Echo a synthetic fixture value.',
    parameters: Type.Object({ value: Type.String() }),
    async execute(_id, args) { return { content: [{ type: 'text', text: `fixture-echo:${args.value}` }], details: {} }; },
  });
}
