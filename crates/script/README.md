# postguy-script

- **Layer:** L3 (enforced by `cargo xtask layers`)

Embedded JS sandbox (rquickjs/QuickJS) with wall-clock, memory, stack and cancel limits;
`Settings.disable_scripts` is the off switch. Everything exposed to scripts is a Rust closure; there
is no JS prelude.

- `pm` — the Postman API (variables, environment, globals, request/response, `pm.test`,
  `pm.expect`, `pm.sendRequest`, `pm.execution`, cookies read access).
- `pg` — a superset of `pm` (every `pm` member, same values) plus Bruno-style variable methods
  (`getVar`/`setVar` on the runtime scope, `getEnvVar`, `getGlobalVar`, `getCollectionVar`,
  `getFolderVar`, `getRequestVar`, `interpolate`, ...), `pg.setNextRequest`/`skipRequest`/
  `stopExecution`, `pg.assets.{base64,text,exists,list}` and `pg.report.{set,get,toObject}`
  (custom run-report columns; 100 keys, 128-char keys, 32 KiB values).
- `bru`, `req`, `res`, `test`, `expect` — Bruno-compatible globals sharing the same Rust code
  (`res` is callable: `res('a.b[0].c')`, `res('..key')`). `bru.sleep`, `bru.cwd`, `bru.runRequest`
  throw "not supported in Postguy scripts".
- `assert` — the Tests-tab assertion evaluator: 28 operators over a JS expression evaluated in the
  post-response context. Order after a response: post-response script, then assertions, then the
  test script (`Context.tests`). Results carry `TestKind::Assertion`; assertions count toward
  `Limits.max_tests`.
- `node` — `Buffer` (utf8, base64, base64url, hex, latin1), an asset-backed CommonJS `require`
  (`./`, `../`, `/` specifiers resolved against the calling module; `.js` and `.json`; cached per
  run; 64 modules and 8 MiB each) and the `fs` (read-only), `path` (POSIX), `buffer` and
  `crypto` (`randomUUID` only) shims. The virtual file system is the collection's Assets folder
  rooted at `/`, reached through `Host::read_asset`/`list_assets`; writes and `..` escapes throw.
  npm packages are not loadable and the error says so.
- Runtime variables (`Variables.runtime`) are visible to scripts and never persisted by this crate.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
