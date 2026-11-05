import { createInterface } from 'node:readline';
import { startApplication } from './application.ts';
const [applicationRoot, dataRoot, control, portText] = process.argv.slice(2);
try {
  if (!applicationRoot || !dataRoot || control !== '--launcher') throw Error('launcher required');
  const input = createInterface({ input: process.stdin });
  const start = await new Promise<boolean>(resolve => { input.once('line', line => resolve(line === 'START'));input.once('close', () => resolve(false)); });
  if (!start) throw Error('launcher absent');
  let ownerGone = false;input.once('close', () => { ownerGone = true; });
  const port = portText === undefined ? 5175 : Number(portText);if (!Number.isInteger(port) || port < 0 || port > 65535) throw Error('port');
  const app = await startApplication({ applicationRoot, dataRoot, port });
  if (ownerGone) { await app.close();throw Error('launcher ended'); }
  input.once('close', () => { void app.close(); });
  process.once('SIGINT', () => { void app.close(); });process.once('SIGTERM', () => { void app.close(); });
  process.stdout.write(JSON.stringify({ url: app.url }) + '\n');
  await app.closed;input.close();
} catch { process.stderr.write('Application startup failed.\n');process.exitCode = 1; }