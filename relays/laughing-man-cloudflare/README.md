# Kota Laughing Man 24/7 Standby Worker

Self-hosted Cloudflare Worker for Laughing Man 24/7 Standby.

This Worker receives Telegram webhooks while the desktop app is unavailable,
returns a small offline notice, and queues messages until Kota is opened again.
The Telegram bot token stays on the user's desktop; the Worker never stores it.
The queue is stored in a single Cloudflare Durable Object created by the Worker
template.

## Deploy

1. Open the Kota Laughing Man settings panel.
2. Click `Open Worker Deploy Page`.
3. Deploy this Worker to your Cloudflare account.
4. Open the generated `workers.dev` URL.
5. Copy that URL back into Kota and click `Connect`.

Kota performs pairing, Telegram `setWebhook`, heartbeat, state sync, and queue
pulling automatically.

## Development

```bash
npm install
npm run dev
npm run deploy
```
