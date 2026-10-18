import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
import kotaExtension from '../../index.ts';

/** Test-only factory probe; never included in the production manifest or resource files. */
export default async function toolProbe(pi: ExtensionAPI) {
  const tools: unknown[] = [];
  const probe: ExtensionAPI = {
    ...pi,
    registerTool(tool) {
      tools.push({ name: tool.name, label: tool.label, description: tool.description, parameters: tool.parameters });
      pi.registerTool(tool);
    },
  };
  await kotaExtension(probe);
  pi.registerCommand('fixture-tools', {
    async handler() {
      pi.sendMessage({
        customType: 'fixture.tools', content: 'Fixture tool metadata.', display: false,
        details: { tools, active: pi.getActiveTools() },
      });
    },
  });
}
