import { spawn } from 'node:child_process';

// The shell waits while the parent verifies this PID's scheduling. exec
// preserves PID, affinity, nice and I/O priority for the exact benchmark.
export const waitingCommand = 'set -e; read -r start; test "$start" = go; exec "$@"';
export function spawnWaiting(binary, args) {
  return spawn('bash', ['-c', waitingCommand, 'redline-checked-worker', binary, ...args],
    { stdio: ['pipe', 'pipe', 'pipe'] });
}
export function releaseWaiting(child) {
  child.stdin.end('go\n');
}
