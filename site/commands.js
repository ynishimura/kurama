// The command reference: the options come from commands.json (the output of
// `kurama agent --json`); the groups, use cases and examples below are
// written by hand from the README and the agent guide.
(() => {
  const GROUPS = [
    { id: "credentials", name: "Credentials", kanji: "鍵", commands: ["env", "exec", "unset", "console", "login", "logout", "token", "status"] },
    { id: "apis", name: "APIs", kanji: "繋", commands: ["api"] },
    { id: "data", name: "Data", kanji: "蔵", commands: ["data", "db", "s3"] },
    { id: "setup", name: "Setup", kanji: "設", commands: ["config", "preset", "init", "completions"] },
    { id: "agents", name: "Agents", kanji: "機", commands: ["agent", "mcp", "audit"] },
  ];

  // What each command is for, in one line.
  const PURPOSE = {
    env: "Put credentials in your current shell, or print them as JSON for a program.",
    exec: "Give one command the credentials; your shell stays unchanged.",
    unset: "Clear what kurama env exported.",
    console: "Open the AWS console as the role.",
    login: "Cache an MFA session or store an OAuth token before unattended work.",
    logout: "Drop cached MFA sessions and stored tokens.",
    token: "Hand a tool the bearer token itself, or compare tokens by fingerprint.",
    status: "See every profile, source and API and whether a person must act first.",
    api: "Call an API with the right credential, or explore its OpenAPI description.",
    data: "Run read-only SQL over CSV, JSONL and Parquet, locally or on S3.",
    db: "Read a database or a SQLite file; change it only with explicit opt-ins.",
    s3: "Walk a bucket, search keys or contents, or preview an object.",
    config: "Read, add to, change and check config.toml without resolving a secret.",
    preset: "Add GitHub, Google, Linear, OpenAI, Slack and more in one command.",
    init: "Load the shell wrapper and completion into zsh.",
    completions: "Print only the completion script, for setups that manage fpath.",
    agent: "Print the contract an agent reads, install Agent Skills, check readiness.",
    mcp: "Serve kurama to an MCP client over stdio, every call as an agent's.",
    audit: "List the api, exec, db and data calls the audit log recorded.",
  };

  const EXAMPLES = {
    env: [
      ["kurama env dev", "assume the role and export the credentials into this shell"],
      ["kurama env prod --readonly", "attach the ReadOnlyAccess policy to the session"],
      ["kurama env dev --json", "credential_process JSON on stdout, no shell changes"],
      ["kurama env github", "export an [auth.*] token into its env_var"],
    ],
    exec: [
      ["kurama exec dev -- aws s3 ls", "one command with the credentials"],
      ["kurama exec prod --readonly -- aws s3 ls", "the same, read-only"],
      ["kurama exec github -- gh api /user", "a token in GITHUB_TOKEN for one command"],
    ],
    unset: [["kurama unset", "remove the variables kurama exported from this shell"]],
    console: [["kurama console prod", "assume the role and open the AWS console"]],
    login: [
      ["kurama login ops", "get and cache an MFA session now (no-op while one is valid)"],
      ["kurama login ops --force", "replace the cached session"],
      ["kurama login github", "OAuth: run the grant and store the token"],
      ["kurama login github --no-browser", "print the authorization URL instead"],
    ],
    logout: [
      ["kurama logout ops", "remove the cached MFA session of the profile's device"],
      ["kurama logout --all", "remove every cached MFA session and stored token"],
    ],
    token: [
      ["kurama token github", "print the access token (refreshed when needed)"],
      ["kurama token github --fingerprint", "sha256 of the token, without printing it"],
    ],
    status: [
      ["kurama status", "profiles, sources and APIs with their state"],
      ["kurama status --json", "the same as one JSON array"],
      ["kurama status --only api --json", "only the APIs"],
      ["kurama status --kind s3 --json", "the [s3.*] connections, nothing reached"],
    ],
    api: [
      ["kurama api github /user --jq .login", "one request with the bearer token"],
      ["kurama api github \"POST /repos/o/r/issues\" -d '{\"title\":\"x\"}' --json", "a method, a body, the envelope"],
      ["kurama api apigw-dev /items", "SigV4-signed with the aws_profile's role"],
      ["kurama api github", "the OpenAPI explorer, on a terminal"],
      ["kurama api github --ops issues", "operations matching \"issues\""],
      ["kurama api github --describe issues/create", "parameters, shapes, scopes, an example"],
      ["kurama api github issues/create -P owner=o -P repo=r -d '{\"title\":\"x\"}'", "call an operation; kurama places the parameters"],
      ["kurama api github --schema issues/create", "the JSON contract an agent builds calls from"],
      ["kurama api github --skill", "an Agent Skill (SKILL.md) for the API"],
      ["kurama api github /user --dry-run --json", "the plan, credentials masked, nothing sent"],
    ],
    data: [
      ["kurama data ./events.jsonl", "preview a local file"],
      ["kurama data ./events.jsonl --query 'SELECT count(*) FROM data' --jq '.rows'", "SQL over one input"],
      ["kurama data --from './orders/*.parquet' --query 'SELECT sum(amount) FROM data' --json", "a glob of Parquet files"],
      ["kurama data --from 's3://my-bucket/orders.parquet' --aws-profile ops --preview data --json", "an S3 object with a role"],
      ["kurama data --from ./orders.csv --describe data --json", "column names and types"],
      ["kurama data --from ./orders.csv --query 'SELECT * FROM data' --export ./orders.parquet", "export the full result"],
    ],
    db: [
      ["kurama db app --tables --json", "what the database holds"],
      ["kurama db app --describe public.orders", "columns, keys and references"],
      ["kurama db app --preview orders --columns id --columns status --max-rows 20", "the first rows of a table"],
      ["kurama db app --query 'SELECT status, count(*) FROM orders GROUP BY status' --jq '.rows'", "one read-only statement"],
      ["kurama db app --query 'SELECT * FROM orders WHERE id = ?' --param 42 --json", "a bound parameter, never SQL"],
      ["kurama db ./fixtures/app.sqlite3 --preview orders", "a SQLite file, no configuration"],
      ["kurama db app", "the database explorer, on a terminal"],
    ],
    s3: [
      ["kurama s3 assets --list --json", "one level under the connection's prefix"],
      ["kurama s3 assets --buckets --json", "the buckets"],
      ["kurama s3 assets s3://example-assets/reports/ --search invoice --json", "keys containing a text"],
      ["kurama s3 assets s3://example-assets/reports/result.json --head --json", "one object's metadata"],
      ["kurama s3 assets s3://example-assets/reports/result.json --preview --bytes 65536 --json", "the start of an object"],
      ["kurama s3 assets s3://example-assets/logs/ --search-content request-id-123 --max-objects 100 --json", "lines containing a text"],
      ["kurama s3 assets", "the S3 explorer, on a terminal"],
    ],
    config: [
      ["kurama config check", "every problem in config.toml at once"],
      ["kurama config show auth.github", "the saved values, literal secrets redacted"],
      ["kurama config add --file new.toml", "append sections, checked whole first"],
      ["kurama config set core.log_level '\"debug\"'", "change one key in place"],
      ["kurama config unset api.example.description", "remove a key so its default applies"],
      ["kurama config remove api.old", "remove a section"],
    ],
    preset: [
      ["kurama preset", "the bundled provider presets"],
      ["kurama preset show github --set secret=op://Agent/kurama-github/credential", "the TOML on stdout, setup steps on stderr"],
      ["kurama preset add github --set secret=op://Agent/kurama-github/credential", "append it to config.toml"],
    ],
    init: [["eval \"$(kurama init zsh)\"", "in ~/.zshrc, after compinit"]],
    completions: [["kurama completions zsh", "only the completion script"]],
    agent: [
      ["kurama agent", "the contract for agents and scripts"],
      ["kurama agent --json", "every subcommand, argument and exit code as JSON"],
      ["kurama agent --kind data --json", "the data CLI's capabilities, no config read"],
      ["kurama agent install", "kurama's Agent Skill and one per API under ~/.claude/skills"],
      ["kurama agent ready --json", "which sources work now, and what a person runs for the rest"],
    ],
    mcp: [["claude mcp add kurama -- kurama mcp", "register kurama with Claude Code"]],
    audit: [
      ["kurama audit --since 12h", "the calls of the last 12 hours"],
      ["kurama audit --json", "the entries as JSON"],
      ["kurama audit --watch", "the calls live, as they are appended"],
    ],
  };

  const SCENES = [
    {
      title: "Work in AWS", kanji: "雲", rows: [
        ["Switch this shell to a role", "kurama env dev", "env"],
        ["Run one tool as a role, leave the shell alone", "kurama exec dev -- terraform plan", "exec"],
        ["Look around prod without the risk of a write", "kurama exec prod --readonly -- aws s3 ls", "exec"],
        ["Open the AWS console as the role", "kurama console prod", "console"],
        ["Sign in once before a long unattended run", "kurama login ops", "login"],
        ["Clear the credentials from this shell", "kurama unset", "unset"],
      ],
    },
    {
      title: "Call an API", kanji: "繋", rows: [
        ["Call an endpoint with the stored token", "kurama api github /user --jq .login", "api"],
        ["Find the operation you need", "kurama api github --ops issues", "api"],
        ["See what an operation takes before calling it", "kurama api github --describe issues/create", "api"],
        ["Call an operation by name", "kurama api github issues/create -P owner=o -P repo=r -d '{\"title\":\"x\"}'", "api"],
        ["Call an IAM-protected AWS endpoint", "kurama api apigw-dev /items", "api"],
        ["Browse an API in a TUI", "kurama api github", "api"],
        ["Give a CLI the token it expects", "kurama exec github -- gh api /user", "exec"],
      ],
    },
    {
      title: "Query data", kanji: "蔵", rows: [
        ["Count rows in a file on S3", "kurama data 's3://my-bucket/orders/2026-08.parquet' --aws-profile ops --query 'SELECT count(*) FROM data'", "data"],
        ["Check a CSV's columns and types", "kurama data --from ./orders.csv --describe data", "data"],
        ["Read a database without a connection string", "kurama db app --tables", "db"],
        ["Try a change and keep nothing", "kurama db app --execute 'UPDATE orders SET status = 1' --rollback", "db"],
        ["Find an object by part of its key", "kurama s3 assets s3://example-assets/reports/ --search invoice", "s3"],
        ["Peek into an object without downloading it", "kurama s3 assets s3://example-assets/reports/result.json --preview", "s3"],
      ],
    },
    {
      title: "Set it up", kanji: "設", rows: [
        ["Add GitHub, Linear, OpenAI…", "kurama preset add github --set secret=op://Agent/kurama-github/credential", "preset"],
        ["Check config.toml after editing it", "kurama config check", "config"],
        ["Change one setting", "kurama config set core.log_level '\"debug\"'", "config"],
        ["Enable the shell wrapper and completion", "eval \"$(kurama init zsh)\"", "init"],
        ["See every source and its state", "kurama status", "status"],
      ],
    },
    {
      title: "Hand it to an agent", kanji: "機", rows: [
        ["Teach an agent kurama and each API", "kurama agent install", "agent"],
        ["Check what works before a run", "kurama agent ready --json", "agent"],
        ["See what a call would do, masked", "kurama api github /user --dry-run --json", "api"],
        ["Serve kurama as MCP tools", "claude mcp add kurama -- kurama mcp", "mcp"],
        ["Review what the agent called", "kurama audit --since 12h", "audit"],
      ],
    },
  ];

  const escape = (text) => String(text).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  const $ = (id) => document.getElementById(id);

  function flagOf(arg) {
    if (arg.positional) return arg.value_name ? `<${arg.value_name}>` : arg.id;
    return [arg.short, arg.long].filter(Boolean).join(", ");
  }
  function synopsis(path, command) {
    const args = command.arguments || [];
    const positional = args.filter((a) => a.positional).map((a) => {
      const name = `<${a.value_name || a.id.toUpperCase()}>${a.multiple ? "..." : ""}`;
      return a.required ? name : `[${name}]`;
    });
    const hasOptions = args.some((a) => !a.positional);
    const hasSubcommands = command.subcommands && Object.keys(command.subcommands).length;
    return ["kurama", ...path, hasSubcommands && !args.length ? "<SUBCOMMAND>" : "", ...positional, hasOptions ? "[OPTIONS]" : ""]
      .filter(Boolean).join(" ");
  }
  // conflicts_with and requires name arguments by id; show them as written.
  function nameOf(id, args) {
    const arg = args.find((a) => a.id === id);
    if (!arg) return id;
    return arg.positional ? `<${arg.value_name || id.toUpperCase()}>` : arg.long || arg.short;
  }
  function details(arg, args) {
    const out = [];
    if (arg.required) out.push('<span class="tag req">required</span>');
    if (arg.multiple) out.push('<span class="tag">repeatable</span>');
    if (arg.values && arg.values.length) out.push(`one of ${arg.values.map((v) => `<code>${escape(v)}</code>`).join(" ")}`);
    if (arg.default && arg.default.length) out.push(`default <code>${escape(arg.default.join(", "))}</code>`);
    const names = (ids) => ids.map((id) => `<code>${escape(nameOf(id, args))}</code>`).join(" ");
    if (arg.conflicts_with && arg.conflicts_with.length) out.push(`not with ${names(arg.conflicts_with)}`);
    if (arg.requires && arg.requires.length) out.push(`needs ${names(arg.requires)}`);
    return out.join("<br>");
  }
  function optionTable(args) {
    if (!args.length) return '<p class="none">No arguments.</p>';
    const rows = args.map((arg) => `
      <tr data-text="${escape([arg.long, arg.short, arg.value_name, arg.help, arg.id].filter(Boolean).join(" ").toLowerCase())}">
        <td class="flag"><code>${escape(flagOf(arg))}</code></td>
        <td class="value">${arg.takes_value && !arg.positional ? `<code>${escape(arg.value_name || "VALUE")}</code>${arg.value_optional ? " (optional)" : ""}` : ""}</td>
        <td class="desc">${escape(arg.help || "")}</td>
        <td class="more">${details(arg, args)}</td>
      </tr>`).join("");
    return `<div class="table-wrap"><table class="opts">
      <thead><tr><th>Option</th><th>Value</th><th>Description</th><th>Details</th></tr></thead>
      <tbody>${rows}</tbody></table></div>`;
  }
  function badges(command) {
    const out = [];
    if (command.json_errors) out.push('<span class="badge ok" title="With --json or --jq a failure is one JSON error document on stderr">JSON errors</span>');
    if (command.request_contract) out.push('<span class="badge" title="Takes a typed JSON request with --request">--request contract</span>');
    return out.join("");
  }
  function exampleList(name) {
    const list = EXAMPLES[name] || [];
    if (!list.length) return "";
    return `<div class="examples">${list.map(([cmd, note], i) => `
      <div class="ex"><pre id="ex-${name}-${i}"><span class="p">$ </span>${escape(cmd)}</pre>
      <span class="note">${escape(note)}</span>
      <button class="copy" type="button" data-copy="ex-${name}-${i}">Copy</button></div>`).join("")}</div>`;
  }
  function subcommandBlocks(name, command) {
    const subs = command.subcommands || {};
    return Object.entries(subs).map(([sub, spec]) => `
      <div class="sub" data-text="${escape(`${name} ${sub} ${spec.about || ""}`.toLowerCase())}">
        <h4><code>kurama ${escape(name)} ${escape(sub)}</code></h4>
        <p>${escape(spec.about || "")}</p>
        <pre class="syn">${escape(synopsis([name, sub], spec))}</pre>
        ${optionTable(spec.arguments || [])}
      </div>`).join("");
  }

  function render(catalog) {
    const commands = catalog.commands;
    const groupOf = {};
    for (const group of GROUPS) for (const name of group.commands) groupOf[name] = group;
    const names = Object.keys(commands);
    const optionCount = names.reduce((n, name) => {
      const c = commands[name];
      return n + (c.arguments || []).length + Object.values(c.subcommands || {}).reduce((m, s) => m + (s.arguments || []).length, 0);
    }, 0);
    const subCount = names.reduce((n, name) => n + Object.keys(commands[name].subcommands || {}).length, 0);
    $("stats").innerHTML = [
      [names.length, "commands"], [subCount, "subcommands"], [optionCount, "arguments"], [catalog.exit_codes.length, "exit codes"],
    ].map(([n, label]) => `<div><b data-to="${n}">0</b><span>${label}</span></div>`).join("");
    $("generated").textContent = `Generated from ${catalog.binary.name} ${catalog.binary.version} (kurama agent --json, schema ${catalog.schema_version}). 鞍馬 Kurama, MIT License.`;

    $("scenes").innerHTML = SCENES.map((scene) => `
      <div class="scene reveal">
        <h3><span class="kanji">${scene.kanji}</span>${escape(scene.title)}</h3>
        <div class="table-wrap"><table class="uc">
          <tbody>${scene.rows.map(([want, cmd, target]) => `
            <tr data-text="${escape(`${want} ${cmd} ${target}`.toLowerCase())}">
              <td class="want">${escape(want)}</td>
              <td><a class="cmdlink" href="#cmd-${target}"><code>${escape(cmd)}</code></a></td>
            </tr>`).join("")}</tbody>
        </table></div>
      </div>`).join("");

    $("glance").innerHTML = GROUPS.map((group) => `
      <div class="glance-group reveal">
        <h3><span class="kanji">${group.kanji}</span>${group.name}</h3>
        ${group.commands.filter((n) => commands[n]).map((n) => `
          <a class="glance-item" href="#cmd-${n}" data-text="${escape(`${n} ${PURPOSE[n] || ""} ${commands[n].about}`.toLowerCase())}">
            <code>${n}</code><span>${escape(PURPOSE[n] || commands[n].about)}</span></a>`).join("")}
      </div>`).join("");

    $("toc").innerHTML = GROUPS.map((group) => `
      <p class="toc-group">${group.name}</p>
      ${group.commands.filter((n) => commands[n]).map((n) => `<a href="#cmd-${n}" data-cmd="${n}">${n}</a>`).join("")}`).join("");

    const ordered = GROUPS.flatMap((g) => g.commands).filter((n) => commands[n]);
    for (const n of names) if (!ordered.includes(n)) ordered.push(n);
    $("commands").innerHTML = ordered.map((name) => {
      const command = commands[name];
      const group = groupOf[name];
      return `
      <article class="cmd" id="cmd-${name}" data-name="${name}" data-text="${escape(`${name} ${command.about} ${PURPOSE[name] || ""} ${(EXAMPLES[name] || []).flat().join(" ")}`.toLowerCase())}">
        <header>
          <div class="cmd-title">
            <span class="group-chip">${group ? group.name : "Other"}</span>
            <h3><code>kurama ${name}</code></h3>
            ${badges(command)}
          </div>
          <p class="about">${escape(command.about)}</p>
          ${PURPOSE[name] ? `<p class="purpose">${escape(PURPOSE[name])}</p>` : ""}
        </header>
        <pre class="syn">${escape(synopsis([name], command))}</pre>
        ${exampleList(name)}
        ${(command.arguments || []).length ? `<h4 class="label">Options</h4>${optionTable(command.arguments)}` : ""}
        ${command.subcommands && Object.keys(command.subcommands).length ? `<h4 class="label">Subcommands</h4>${subcommandBlocks(name, command)}` : ""}
      </article>`;
    }).join("");

    $("exits").innerHTML = `<thead><tr><th>Exit</th><th>Meaning</th><th>Error codes</th></tr></thead><tbody>${
      catalog.exit_codes.map((e) => `<tr><td class="code-n">${e.exit}</td><td>${escape(e.meaning)}</td><td>${
        (e.error_codes || []).map((c) => `<span class="ecode">${escape(c)}</span>`).join("") || "&ndash;"}</td></tr>`).join("")}</tbody>`;
  }

  function wire() {
    for (const button of document.querySelectorAll("button.copy")) {
      button.addEventListener("click", async () => {
        const text = document.getElementById(button.dataset.copy).textContent.replace(/^\$ /gm, "").trim();
        await navigator.clipboard.writeText(text);
        button.textContent = "Copied";
        button.classList.add("done");
        setTimeout(() => { button.textContent = "Copy"; button.classList.remove("done"); }, 1500);
      });
    }

    const revealer = new IntersectionObserver((entries) => {
      for (const entry of entries) if (entry.isIntersecting) { entry.target.classList.add("in"); revealer.unobserve(entry.target); }
    }, { threshold: 0.1 });
    document.querySelectorAll(".reveal").forEach((el) => revealer.observe(el));

    // Count the stats up once.
    for (const b of document.querySelectorAll("#stats b")) {
      const to = Number(b.dataset.to);
      const start = performance.now();
      (function tick(now) {
        const k = Math.min((now - start) / 1100, 1);
        b.textContent = Math.round(to * (1 - Math.pow(1 - k, 3)));
        if (k < 1) requestAnimationFrame(tick);
      })(start);
    }

    // Scrollspy for the sidebar.
    const links = new Map([...document.querySelectorAll("#toc a")].map((a) => [a.dataset.cmd, a]));
    const spy = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        links.forEach((a) => a.classList.remove("on"));
        const link = links.get(entry.target.dataset.name);
        if (!link) continue;
        link.classList.add("on");
        // Keep the link in view inside the sidebar without moving the page.
        const toc = link.closest(".toc");
        if (toc.scrollWidth > toc.clientWidth) toc.scrollTo({ left: link.offsetLeft - 40, behavior: "smooth" });
        else if (link.offsetTop < toc.scrollTop || link.offsetTop > toc.scrollTop + toc.clientHeight - 40) toc.scrollTo({ top: link.offsetTop - 80 });
      }
    }, { rootMargin: "-20% 0px -70% 0px" });
    document.querySelectorAll(".cmd").forEach((el) => spy.observe(el));

    // Search: filter use cases, the overview, commands and their rows; mark matches.
    const input = $("q");
    function filter() {
      const words = input.value.toLowerCase().trim().split(/\s+/).filter(Boolean);
      const hit = (el) => words.every((w) => (el.dataset.text || "").includes(w));
      let any = false;
      document.querySelectorAll(".uc tr").forEach((tr) => { tr.hidden = words.length > 0 && !hit(tr); });
      document.querySelectorAll(".scene").forEach((s) => { s.hidden = words.length > 0 && !s.querySelector("tr:not([hidden])"); });
      document.querySelectorAll(".glance-item").forEach((a) => { a.hidden = words.length > 0 && !hit(a); });
      document.querySelectorAll(".glance-group").forEach((g) => { g.hidden = words.length > 0 && !g.querySelector(".glance-item:not([hidden])"); });
      document.querySelectorAll(".cmd").forEach((cmd) => {
        const rows = [...cmd.querySelectorAll(".opts tbody tr")];
        const subs = [...cmd.querySelectorAll(".sub")];
        if (!words.length) {
          rows.forEach((r) => { r.hidden = false; r.classList.remove("match"); });
          subs.forEach((s) => { s.hidden = false; });
          cmd.hidden = false;
          any = true;
          return;
        }
        const whole = hit(cmd);
        rows.forEach((r) => { const m = hit(r); r.classList.toggle("match", m); r.hidden = !whole && !m; });
        subs.forEach((s) => { s.hidden = !whole && !hit(s) && !s.querySelector(".opts tr.match"); });
        cmd.hidden = !whole && !rows.some((r) => !r.hidden);
        if (!cmd.hidden) any = true;
        links.get(cmd.dataset.name)?.classList.toggle("dim", cmd.hidden);
      });
      if (!words.length) links.forEach((a) => a.classList.remove("dim"));
      $("empty").hidden = any;
    }
    input.addEventListener("input", filter);
    addEventListener("keydown", (e) => {
      if (e.key === "/" && document.activeElement !== input) { e.preventDefault(); input.focus(); }
      if (e.key === "Escape" && document.activeElement === input) { input.value = ""; filter(); input.blur(); }
    });
  }

  fetch("commands.json")
    .then((response) => response.json())
    .then((catalog) => { render(catalog); wire(); if (location.hash) document.querySelector(location.hash)?.scrollIntoView(); });
})();
