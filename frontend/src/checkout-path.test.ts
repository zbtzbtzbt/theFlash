// The frontend tests pass in a checkout whose path has Chinese characters
// and spaces, like `../theFlash-验收` (#49). URL.pathname keeps such
// characters percent-encoded, so a test that takes a file path from it asks
// for `…/theFlash-%E9%AA%8C%E6%94%B6/…`, which does not exist;
// fileURLToPath decodes them. The tests below run the frontend tests again
// from a copy of this checkout under such a path.
import { execFile } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { stripVTControlCharacters } from "node:util";
import { expect, it } from "vitest";
import { tempHome } from "./test-support.js";

const probing = process.env.THEFLASH_CHECKOUT_PATH_PROBE === "1";
const repo = fileURLToPath(new URL("../..", import.meta.url));
const name = "theFlash 验收";

// Runs only in the copy, and fails that run unless the test files really
// are there rather than reached through a link back to this checkout.
it.runIf(probing)("probe: the test files run from the copy", () => {
  expect(fileURLToPath(import.meta.url)).toContain(`${sep}${name}${sep}`);
});

// A copy of this checkout under `<dir>/theFlash 验收`. frontend/ is copied,
// not linked: vitest follows links, and a test file reached through one
// would see this checkout's path. What the tests only find from their own
// location (the fixtures, the built theflash, each installed package) is
// linked, and the link's own path is in the copy. node_modules itself is a
// new directory, so the caches vitest keeps there go away with the copy.
function checkout(dir: string, built: boolean): string {
  const root = join(dir, name);
  const modules = join(repo, "frontend", "node_modules");
  cpSync(join(repo, "frontend"), join(root, "frontend"), {
    recursive: true,
    filter: (path) => path !== modules,
  });
  mkdirSync(join(root, "frontend", "node_modules"));
  for (const entry of readdirSync(modules)) {
    if (!entry.startsWith("."))
      symlinkSync(join(modules, entry), join(root, "frontend", "node_modules", entry));
  }
  mkdirSync(join(root, "backend", "target", "debug"), { recursive: true });
  symlinkSync(join(repo, "backend", "testdata"), join(root, "backend", "testdata"));
  if (built)
    symlinkSync(
      join(repo, "backend", "target", "debug", "theflash"),
      join(root, "backend", "target", "debug", "theflash"),
    );
  return join(root, "frontend");
}

// `vitest run` in `cwd`, as a developer there would start it, under a
// temporary HOME so nothing it starts reads or writes the real one.
function vitest(cwd: string, ...args: string[]): Promise<{ code: number; output: string }> {
  const home = tempHome();
  const env: NodeJS.ProcessEnv = { ...process.env, THEFLASH_CHECKOUT_PATH_PROBE: "1" };
  return new Promise<{ code: number; output: string }>((resolve) => {
    execFile(
      process.execPath,
      [join(cwd, "node_modules", "vitest", "vitest.mjs"), "run", ...args],
      { cwd, env, timeout: 100_000 },
      (error, stdout, stderr) =>
        resolve({
          code: error ? (typeof error.code === "number" ? error.code : -1) : 0,
          output: `${stdout}${stderr}`,
        }),
    );
  }).finally(home.remove);
}

async function inCopy<T>(built: boolean, run: (frontend: string) => Promise<T>): Promise<T> {
  const dir = mkdtempSync(join(tmpdir(), "theflash-checkout-path-"));
  try {
    return await run(checkout(dir, built));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function expectCopyPassed(run: { code: number; output: string }): void {
  const failed = Array.from(
    stripVTControlCharacters(run.output).matchAll(/^\s*FAIL\s+(.+)$/gm),
    (match) => match[1]?.trim() ?? "",
  ).filter(Boolean);
  const list =
    failed.length > 0
      ? `前端测试在路径含中文和空格的 checkout 副本里失败，没过的测试：\n${failed.map((item) => `  - ${item}\n`).join("")}`
      : "前端测试在路径含中文和空格的 checkout 副本里失败。\n";
  expect(
    run.code,
    `${list}这些测试在原 checkout 里也失败的话，先修它们，原因不在路径；` +
      "只在副本里失败，多半是用了 `URL.pathname`（#49）。从 import.meta.url 得到文件路径要用 " +
      "fileURLToPath，不能用 URL.pathname：它保留百分号编码，中文和空格变成 %E9%AA%8C、%20，" +
      `这个路径不存在。那次运行的输出：\n${run.output}`,
  ).toBe(0);
}

it.skipIf(probing)(
  "lists failed tests before the checkout-path hint so ordinary regressions are not misattributed to URL.pathname",
  () => {
    const output = [
      " RUN  v5.0.4 /tmp/theFlash 验收/frontend",
      "",
      " ❯ src/app.test.ts (18 tests | 1 failed) 210ms",
      "   × the five status styles have distinct foreground and background colors 18ms",
      "",
      "⎯⎯⎯⎯⎯⎯⎯ Failed Suites 1 ⎯⎯⎯⎯⎯⎯⎯",
      "",
      " FAIL  src/query.test.ts [ src/query.test.ts ]",
      "Error: spawn /tmp/theFlash%20%E9%AA%8C%E6%94%B6/backend/target/debug/theflash ENOENT",
      "",
      "⎯⎯⎯⎯⎯⎯⎯ Failed Tests 1 ⎯⎯⎯⎯⎯⎯⎯",
      "",
      " \u001b[41m\u001b[1m FAIL \u001b[22m\u001b[49m src/app.test.ts > the five status styles have distinct foreground and background colors",
      "AssertionError: “未开售”没有自己的文字颜色: expected 'rgb(236, 239, 233)' not to be 'rgb(236, 239, 233)'",
      "",
      " Test Files  2 failed | 4 passed (6)",
      "      Tests  1 failed | 35 passed | 3 skipped (39)",
    ].join("\n");
    let message = "";
    try {
      expectCopyPassed({ code: 1, output });
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    }
    const [summary = "", raw = ""] = message.split("那次运行的输出：\n");
    const suite = "src/query.test.ts [ src/query.test.ts ]";
    const test =
      "src/app.test.ts > the five status styles have distinct foreground and background colors";
    const guidance =
      "这些测试在原 checkout 里也失败的话，先修它们，原因不在路径；只在副本里失败，多半是用了 `URL.pathname`（#49）";
    expect(summary).toContain(suite);
    expect(summary).toContain(test);
    expect(summary).toContain(guidance);
    expect(summary.indexOf(suite)).toBeLessThan(summary.indexOf(guidance));
    expect(summary.indexOf(test)).toBeLessThan(summary.indexOf(guidance));
    expect(raw).toContain(output);
  },
);

it.skipIf(probing)(
  "the frontend tests pass in a checkout whose path has Chinese characters and a space",
  async () => {
    const run = await inCopy(true, (frontend) => vitest(frontend));
    expectCopyPassed(run);
  },
  120_000,
);

// A checkout where theflash is not built yet: app.test.ts's setup has
// started its servers when its first query fails.
it.skipIf(probing)(
  "a test file whose setup fails partway reports that failure and nothing else",
  async () => {
    const run = await inCopy(false, (frontend) => vitest(frontend, "src/app.test.ts"));
    expect(run.code, run.output).toBe(1);
    expect(run.output).toContain("本地查询程序不可用");
    expect(run.output, "准备没做完时，清理代码自己又报了一个错，盖住了真正的原因").not.toMatch(
      /TypeError|Cannot read properties of undefined/,
    );
  },
  120_000,
);
