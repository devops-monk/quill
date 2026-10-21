import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { detectIssueCommand, handleIssueCommand } from "./issue-commands.mjs";
import { writeContributorSnapshot } from "../../docs/scripts/contributor-snapshot.mjs";

function workflow(name) {
  return readFileSync(new URL(`../workflows/${name}.yml`, import.meta.url), "utf8");
}

function job(content, name) {
  const jobs = content.slice(content.indexOf("\njobs:\n") + 7);
  const definitions = [...jobs.matchAll(/^  ([\w-]+):\s*$/gm)];
  const index = definitions.findIndex((match) => match[1] === name);
  assert.ok(index >= 0, `missing job ${name}`);
  return jobs.slice(definitions[index].index, definitions[index + 1]?.index ?? jobs.length);
}

function evaluateCondition(content, github) {
  const folded = content.match(/^    if: >-\n((?:      [^\n]*\n)+)/m);
  const expression = (folded?.[1] ?? content.match(/^    if: (.+)$/m)?.[1])?.trim();
  assert.ok(expression, "missing job condition");
  const evaluate = new Function("github", "contains", `return (${expression.replace(/^\$\{\{\s*|\s*\}\}$/g, "")});`);
  return evaluate(github, (search, item) =>
    String(search ?? "")
      .toLowerCase()
      .includes(String(item).toLowerCase()),
  );
}

function commentEvent(body, { userType = "User", login = "contributor", pullRequest = false } = {}) {
  return {
    repository_owner: "t8y2",
    event: {
      issue: { pull_request: pullRequest ? { url: "https://api.github.com/repos/t8y2/dbx/pulls/123" } : undefined },
      comment: { body, user: { type: userType, login } },
    },
  };
}

test("GitHub script selection excludes planner tests only when fast checks own them", () => {
  const content = job(workflow("ci"), "github-scripts");
  const script = content.match(/run: \|\n((?:          [^\n]*\n)+)/)[1].replace(/^          /gm, "");
  const repository = new URL("../../", import.meta.url);
  const files = readdirSync(new URL(".github/scripts/", repository)).filter((file) => file.endsWith(".test.mjs"));
  for (const fast of ["true", "false", ""]) {
    const result = spawnSync("bash", ["-e", "-o", "pipefail", "-c", `node() { printf '%s\\n' "$@"; }\n${script}`], {
      cwd: repository,
      encoding: "utf8",
      env: { ...process.env, FAST_CHECKS: fast },
    });
    assert.equal(result.status, 0, result.stderr);
    const selected = result.stdout.trim().split("\n").slice(1).sort();
    const expected = files
      .filter((file) => fast !== "true" || !file.startsWith("ci-"))
      .map((file) => `.github/scripts/${file}`)
      .sort();
    assert.deepEqual(selected, expected);
    assert.ok(selected.includes(".github/scripts/actions-efficiency.test.mjs"));
  }
});

async function snapshotFixture(context) {
  const directory = await mkdtemp(join(tmpdir(), "dbx-contributor-snapshot-"));
  context.after(() => rm(directory, { recursive: true, force: true }));
  return join(directory, "data", "contributors.json");
}

function snapshot() {
  return { repository: "t8y2/dbx", generatedAt: "2026-10-01T18:00:00.000Z", stars: 10, contributors: [{ login: "contributor", commits: 3, mergedPullRequests: 1 }] };
}

test("a missing contributor snapshot is created and timestamp-only refreshes preserve exact bytes", async (context) => {
  const outputPath = await snapshotFixture(context);
  const original = snapshot();
  assert.equal(await writeContributorSnapshot(outputPath, original), true);
  const originalBytes = await readFile(outputPath, "utf8");
  assert.equal(originalBytes, `${JSON.stringify(original, null, 2)}\n`);
  const refreshed = { ...original, generatedAt: "2026-10-08T18:00:00.000Z" };
  assert.equal(await writeContributorSnapshot(outputPath, refreshed), false);
  assert.equal(await readFile(outputPath, "utf8"), originalBytes);
  assert.equal(refreshed.generatedAt, "2026-10-08T18:00:00.000Z");
});

test("actual contributor, star and repository changes update the snapshot", async (context) => {
  const outputPath = await snapshotFixture(context);
  const original = snapshot();
  for (const updated of [
    { ...original, stars: 11 },
    { ...original, repository: "other/dbx" },
    { ...original, contributors: [{ ...original.contributors[0], commits: 4 }] },
    { ...original, contributors: [] },
  ]) {
    await writeContributorSnapshot(outputPath, original);
    const refreshed = { ...updated, generatedAt: "2026-10-08T18:00:00.000Z" };
    assert.equal(await writeContributorSnapshot(outputPath, refreshed), true);
    assert.deepEqual(JSON.parse(await readFile(outputPath, "utf8")), refreshed);
  }
});

test("invalid contributor snapshots and filesystem errors are not silently overwritten", async (context) => {
  const outputPath = await snapshotFixture(context);
  await writeContributorSnapshot(outputPath, snapshot());
  await writeFile(outputPath, "invalid JSON");
  await assert.rejects(writeContributorSnapshot(outputPath, snapshot()), SyntaxError);
  assert.equal(await readFile(outputPath, "utf8"), "invalid JSON");
  await assert.rejects(writeContributorSnapshot(join(outputPath, "invalid"), snapshot()), { code: "ENOTDIR" });
});
