// A frontend test that forgets its fixture server fails the run and names
// what it tried to reach, instead of reaching damai.cn, whether or not it
// called offline(): vitest.config.ts puts every run behind the tripwire
// (the frontend side of decision 0003, #34).
import { execFile } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { get } from "node:https";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { loadEvent } from "./query.js";

const probing = process.env.THEFLASH_OFFLINE_PROBE === "1";

// What a test that forgot both its fixture server and offline() does, with
// each kind of client a frontend test has: theflash, Node's fetch and
// node:https. The stand-in for damai.cn is 0.0.0.0, which is not loopback,
// so clients ask the proxy for it as for any other host, while a direct
// connection to it stays on this machine and is refused: the probe reaches
// nothing outside even with the tripwire broken. Skipped in a normal run;
// the test below runs it in a run of its own, because the tripwire fails
// the whole run it fires in.
it.runIf(probing)("probe: requests for another machine, with no offline()", async () => {
  process.env.THEFLASH_DAMAI_BASE_URL = "https://0.0.0.0:1";
  await loadEvent("1086791513717", new AbortController().signal).catch(() => {});
  await fetch("http://0.0.0.0:2/").catch(() => {});
  await new Promise<void>((resolve) => {
    get("https://0.0.0.0:3/", (res) => res.resume().on("end", resolve)).on("error", () =>
      resolve(),
    );
  });
});

it.skipIf(probing)(
  "a test that forgot its fixture server fails the run and names what it tried",
  async () => {
    // Only the probe run's own setup may catch it, not this run's tripwire
    // or the dead proxy of make check.
    const env: NodeJS.ProcessEnv = { ...process.env, THEFLASH_OFFLINE_PROBE: "1" };
    for (const key of [
      "HTTP_PROXY",
      "HTTPS_PROXY",
      "ALL_PROXY",
      "http_proxy",
      "https_proxy",
      "all_proxy",
      "NO_PROXY",
      "no_proxy",
      "NODE_USE_ENV_PROXY",
    ])
      delete env[key];
    // A HOME of its own, so nothing the probe run starts reads or writes
    // the real one.
    const home = mkdtempSync(join(tmpdir(), "theflash-offline-probe-"));
    env.HOME = home;
    delete env.XDG_STATE_HOME;
    const vitest = fileURLToPath(new URL("../node_modules/vitest/vitest.mjs", import.meta.url));
    const run = await new Promise<{ code: number; output: string }>((resolve) => {
      execFile(
        process.execPath,
        [vitest, "run", "src/offline.test.ts"],
        { cwd: fileURLToPath(new URL("..", import.meta.url)), env },
        (error, stdout, stderr) =>
          resolve({
            code: error ? (typeof error.code === "number" ? error.code : -1) : 0,
            output: `${stdout}${stderr}`,
          }),
      );
    }).finally(() => rmSync(home, { recursive: true, force: true }));
    expect(run.code, `忘了 fixture 服务器的测试没有让运行失败：\n${run.output}`).toBe(1);
    expect(run.output).toContain("测试试图访问真实大麦");
    // What the tripwire saw from theflash, fetch and node:https.
    for (const hit of ["0.0.0.0:1", "http://0.0.0.0:2/", "0.0.0.0:3"])
      expect(run.output, `陷阱没有拦下 ${hit}：\n${run.output}`).toContain(`  ${hit}\n`);
  },
  60_000,
);
