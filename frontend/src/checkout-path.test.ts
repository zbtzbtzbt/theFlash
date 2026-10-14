// The frontend tests pass in a checkout whose path has Chinese characters,
// spaces, and percent signs, like `../theFlash 验收 100%` (#49, #104).
// URL.pathname keeps such characters percent-encoded, so a test that takes a
// file path from it asks for `…/theFlash%20%E9%AA%8C%E6%94%B6%20100%25/…`,
// which does not exist; fileURLToPath decodes them. The tests below run the
// frontend tests again from a copy of this checkout under such a path.
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
const name = "theFlash 验收 100%";

// Runs only in the copy, and fails that run unless the test files really
// are there rather than reached through a link back to this checkout.
it.runIf(probing)("probe: the test files run from the copy", () => {
  expect(fileURLToPath(import.meta.url)).toContain(`${sep}${name}${sep}`);
});

// A copy of this checkout under `<dir>/theFlash 验收 100%`. frontend/ is copied,
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
function vitest(
  cwd: string,
  ...args: string[]
): Promise<{ code: number; output: string; path: string }> {
  const home = tempHome();
  const env: NodeJS.ProcessEnv = { ...process.env, THEFLASH_CHECKOUT_PATH_PROBE: "1" };
  return new Promise<{ code: number; output: string; path: string }>((resolve) => {
    execFile(
      process.execPath,
      [join(cwd, "node_modules", "vitest", "vitest.mjs"), "run", ...args],
      { cwd, env, timeout: 100_000 },
      (error, stdout, stderr) =>
        resolve({
          code: error ? (typeof error.code === "number" ? error.code : -1) : 0,
          output: `${stdout}${stderr}`,
          path: cwd,
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

function hasUnsupportedVitePathChar(
  run: { output: string; path?: string },
  clean: string,
): boolean {
  if (run.path && /[#?*]/.test(run.path)) return true;
  if (
    /The (?:project root|config path) contains the (?:"[#?*]"(?: and )?)+ characters?\b/.test(clean)
  ) {
    return true;
  }
  return (
    /ERR_LOAD_URL|UNRESOLVED_ENTRY/.test(clean) &&
    /(?:resolved id:\s*|config from\s+|root\b[^\n]*)\S*[#?*]\S*/.test(clean)
  );
}

function expectCopyPassed(run: { code: number; output: string; path?: string }): void {
  const clean = stripVTControlCharacters(run.output);
  const failed = Array.from(
    clean.matchAll(/^\s*FAIL\s+(.+)$/gm),
    (match) => match[1]?.trim() ?? "",
  ).filter(Boolean);
  const list =
    failed.length > 0
      ? `前端测试在路径含中文、空格和百分号的 checkout 副本里失败，没过的测试：\n${failed.map((item) => `  - ${item}\n`).join("")}`
      : "前端测试在路径含中文、空格和百分号的 checkout 副本里失败。\n";
  const hint = hasUnsupportedVitePathChar(run, clean)
    ? "这是 Vite / Rolldown 上游对项目路径含 `#`、`?`、`*` 的 URL 解析限制（#104）：" +
      "上游将根路径、配置文件和模块 ID 按 URL 处理，遇到 `#` 或 `?` 会截断路径。" +
      "应改用不含 `#`、`?`、`*` 的工作目录名（中文、空格和 `%` 均受支持）。"
    : "这些测试在原 checkout 里也失败的话，先修它们，原因不在路径；" +
      "只在副本里失败，多半是用了 `URL.pathname`（#49）。从 import.meta.url 得到文件路径要用 " +
      "fileURLToPath，不能用 URL.pathname：它保留百分号编码，中文、空格和百分号变成 %E9%AA%8C、%20、%25，" +
      "这个路径不存在。";
  expect(run.code, `${list}${hint}那次运行的输出：\n${run.output}`).toBe(0);
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
  "diagnoses Vite's # / ? / * checkout-path restriction (#104) instead of blaming URL.pathname (#49)",
  () => {
    const hashOutput = [
      'The project root contains the "#" and "?" characters (/tmp/hash#only/frontend), which may not work when running Vite.',
      "No test files found, exiting with code 1",
      "Error: Failed to load url /src/test-support.ts (resolved id: /tmp/hash#only/frontend/src/test-support.ts). Does the file exist?",
      "Serialized Error: { code: 'ERR_LOAD_URL' }",
    ].join("\n");
    const questionOutput = [
      'The config path contains the "?" character (/tmp/question?only/frontend/vitest.config.ts), which may not work when running Vite. Consider renaming the file without the characters.',
      "[UNRESOLVED_ENTRY] Cannot resolve entry module vitest.config.ts.",
    ].join("\n");
    const errLoadUrlOnly = [
      "Error: Failed to load url /src/test-support.ts (resolved id: /tmp/star*only/frontend/src/test-support.ts). Does the file exist?",
      "Serialized Error: { code: 'ERR_LOAD_URL' }",
    ].join("\n");
    for (const run of [
      { code: 1, output: hashOutput },
      { code: 1, output: questionOutput },
      { code: 1, output: errLoadUrlOnly },
      {
        code: 1,
        output: "No test files found, exiting with code 1",
        path: "/tmp/hash#only/frontend",
      },
    ]) {
      let message = "";
      try {
        expectCopyPassed(run);
      } catch (error) {
        message = error instanceof Error ? error.message : String(error);
      }
      const [summary = "", raw = ""] = message.split("那次运行的输出：\n");
      expect(summary).toContain(
        "Vite / Rolldown 上游对项目路径含 `#`、`?`、`*` 的 URL 解析限制（#104）",
      );
      expect(summary).toContain("应改用不含 `#`、`?`、`*` 的工作目录名");
      expect(summary).not.toContain("URL.pathname");
      expect(summary).not.toContain("#49");
      expect(raw).toContain(run.output);
    }
  },
);

it.skipIf(probing)(
  "the frontend tests pass in a checkout whose path has Chinese characters, spaces, and a percent sign",
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
