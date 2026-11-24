const test = require("node:test");
const assert = require("node:assert");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");
const { checkExample, requiredSecrets } = require("./check-env-example");

const req = new Set(["A_SECRET"]);
const script = path.join(__dirname, "check-env-example.js");
test("matching keys pass", () => assert.deepEqual(checkExample("# c\nA_SECRET=\n", req), []));
test("missing key fails", () => assert.match(checkExample("", req)[0], /missing/));
test("stale key fails", () => assert.match(checkExample("A_SECRET=\nB_KEY=", req)[0], /stale/));
test("duplicate key fails", () => assert.match(checkExample("A_SECRET=\nA_SECRET=", req)[0], /duplicate/));
test("malformed line fails", () => assert.match(checkExample("A_SECRET=\nnope", req)[0], /malformed/));
test("values fail without being leaked", () => {
  const errors = checkExample("A_SECRET=hunter2", req);
  assert.match(errors[0], /must not have a value/);
  assert.ok(!errors.join().includes("hunter2"));
});
test("no detected secrets fails closed", () => assert.equal(checkExample("", new Set()).length, 1));
test("detects read styles and ignores config and tests", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "env-"));
  fs.mkdirSync(path.join(dir, "tests"));
  fs.writeFileSync(path.join(dir, "a.ts"), 'process.env.A_SECRET; process.env["B_KEY"]; env("C_TOKEN", ""); process.env.PORT');
  fs.writeFileSync(path.join(dir, "b.rs"), 'std::env::var("D_PASSWORD")');
  fs.writeFileSync(path.join(dir, "tests", "t.ts"), "process.env.E_SECRET");
  assert.deepEqual([...requiredSecrets(dir)].sort(), ["A_SECRET", "B_KEY", "C_TOKEN", "D_PASSWORD"]);
  fs.rmSync(dir, { recursive: true });
});
test("repo example is in sync; missing file fails", () => {
  assert.equal(spawnSync("node", [script]).status, 0);
  assert.equal(spawnSync("node", [script, "--example", "/nonexistent"]).status, 1);
});
