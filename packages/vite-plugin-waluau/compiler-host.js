import { spawn } from 'node:child_process';

function commandLabel(command, args) {
  return [command, ...args].join(' ');
}

/**
 * Keep one `waluau --server` process alive and multiplex CLI-compatible build
 * requests over newline-delimited JSON.
 */
export function createCompilerHost({ command, args = [], cwd }) {
  let child = null;
  let childExit = null;
  let nextId = 1;
  let closed = false;
  const pending = new Map();

  function rejectPending(error) {
    for (const { reject } of pending.values()) reject(error);
    pending.clear();
  }

  function start() {
    if (closed) throw new Error('Waluau compiler host is closed');
    if (child != null) return child;

    const process = spawn(command, args, {
      cwd,
      stdio: ['pipe', 'pipe', 'inherit'],
    });
    child = process;
    let stdout = '';
    childExit = new Promise((resolveExit) => {
      process.once('exit', (code, signal) => {
        if (child === process) child = null;
        const suffix = signal == null ? `exit code ${code}` : `signal ${signal}`;
        if (pending.size > 0) {
          rejectPending(new Error(`${commandLabel(command, args)} stopped with ${suffix}`));
        }
        resolveExit();
      });
    });
    process.once('error', (error) => {
      if (child === process) child = null;
      rejectPending(error);
    });
    process.stdout.setEncoding('utf8');
    process.stdout.on('data', (chunk) => {
      stdout += chunk;
      while (true) {
        const newline = stdout.indexOf('\n');
        if (newline < 0) break;
        const line = stdout.slice(0, newline).trim();
        stdout = stdout.slice(newline + 1);
        if (line.length === 0) continue;
        let response;
        try {
          response = JSON.parse(line);
        } catch (error) {
          rejectPending(new Error(
            `Invalid response from ${commandLabel(command, args)}: ${line}`,
            { cause: error },
          ));
          process.kill();
          return;
        }
        const request = pending.get(response.id);
        if (request == null) continue;
        pending.delete(response.id);
        if (response.ok) request.resolve(response);
        else request.reject(new Error(response.error ?? 'Waluau compilation failed'));
      }
    });
    return process;
  }

  function build(buildArgs) {
    const process = start();
    const id = nextId++;
    return new Promise((resolveBuild, rejectBuild) => {
      pending.set(id, { resolve: resolveBuild, reject: rejectBuild });
      process.stdin.write(`${JSON.stringify({ id, args: buildArgs })}\n`, (error) => {
        if (error == null) return;
        pending.delete(id);
        rejectBuild(error);
      });
    });
  }

  async function stopCurrent() {
    const process = child;
    const exited = childExit;
    if (process == null || exited == null) return;
    process.stdin.end();
    await exited;
  }

  return {
    build,
    async restart() {
      await stopCurrent();
    },
    async close() {
      closed = true;
      await stopCurrent();
    },
  };
}
