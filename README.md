# Universal Prompt Language

UPL is a language to define dynamic prompts which contain variables, conditionals and loops. This allows you to dynamically construct complex prompts, [see the language specification](upl-spec/upl-1.0-rfc.md). UPL files are just `.txt` or `.upl` files, see examples at `samples/` directory.

UPL variables are strictly typed (`string`, `long_string`, `number`, `boolean`,
`list`, `object`, `object_shape`, `option_single`, `option_multi`). An `object` is
asked to the user as a parameter; an `object_shape` declares a reusable object
shape that is never prompted for on its own — it is only collected at the site
that references it (a `list`, `option_single`, or `option_multi` element, or
an `object`/field that reuses it via `type: <name>`). See §3 of the RFC for details.

![preview](https://github.com/DavidValin/universal-prompt-language/raw/master/preview.gif)

This project contains:

1. `upl specification`
2. terminal `prompts browser` with tag support
3. terminal `prompts builder`, to construct a dynamic prompt interactively by filling in the input variables
4. terminal `prompt editor` with syntax highlight
5. `build history` with resume and rebuild support
6. UPL `repository server`, to create a centralized repository of upl prompts
7. UPL `repository client`, to publish/download upl prompts to/from a an upl repository

All operations are integrated in a single cli command called `upl`.

### Quickview

* Specification: [`upl-spec/upl-1.0-rfc.pdf`](upl-spec/upl-1.0-rfc.md)
* Prompt Browser: `upl <prompts_folder>` (or `upl` alone to browse `~/.upl`)
* Prompt Builder: `upl b <prompt_file.txt>` (builds a single file directly)
* Prompt Editor: `upl init` (create a new prompt from the skeleton)
* Repository server: `upl start-repository`
* Repository client (push): `upl push a_user/my_nice_prompt`
* Repository client (push): `upl pull a_user/my_nice_prompt`

`upl` is the cli interface which allows you to navigate through the prompts and build them by filling in the variables. `upl` allows you to create a repository server to store the prompts and it also to communicate with a `upl` repository to `push` and `pull` prompts from it.

## Install

```bash
make release
sudo make install
```

## First-run setup

On its very first run `upl` checks for the user library at `~/.upl`. If that
folder does not exist yet, `upl` creates it and seeds it with a small bundled
starter library so you can start browsing and building right away:

- `~/.upl/prompts/` — a set of sample prompts (`analyze_argument.txt`,
  `create_a_plan.txt`, `create_rest_api.txt`, `explain_subject.txt`,
  `implement_user_story.txt`, `review_article.txt`,
  `teach_foundations.txt`).
- `~/.upl/tags_db` — a pre-built tag store associating those sample prompts
  with their tags (e.g. `software development`, `planning`, `understand`,
  `analysis`, `review`, `teaching`).

These samples are compiled into the `upl` binary itself, so they are placed
even on a machine with no network access. The seeding happens only once: if
`~/.upl` already exists (for example because you have already customized your
library), nothing is overwritten.

## Browse prompts

Browse a prompt library with an interactive TUI to pick one.

```bash
# Browse the default library at ~/.upl/prompts
upl
# or: upl build

# Browse a specific folder
upl <prompts_folder>
# or: upl build <prompts_folder>
```

From the browser you can press `e` on a prompt to open it in the [Prompt
Editor](#prompt-editor), or `n` to create a new one from the skeleton.
Press `Ctrl+H` to open the [Build History](#build-history) sidebar and
resume or rebuild a previous build.

## Build and Run a prompt from an upl prompt

Build a single prompt file directly by filling in its variables interactively
(no TUI library browsing needed).

```bash
# Build a specific prompt file directly (prompts for each variable)
upl build samples/create_a_plan.txt
# or: upl b samples/create_a_plan.txt

# Build non-interactively using its declared defaults
upl build --no-input samples/explain_subject.txt
```

Once a prompt is built, you will see the output in screen which you can copy.
You can also pass the prompt to another bash command via pipe, for example [ai-chat](https://www.github.com/sigoden/aichat).

```bash
upl b create_a_plan.txt | aichat
```

or to browse+build in one go and pass the built prompt:

```bash
upl | aichat
```

## Build a prompt from a JSON file

Build a prompt non-interactively by supplying parameter values in a JSON file.
The values are validated against the prompt's declared parameters (types,
`opts`, `ofields`) and, if valid, used to render the final prompt to stdout.
Missing parameters fall back to declared `def:` defaults.

```bash
# Build by prompt name (resolved from ~/.upl/prompts/)
upl build-from-json create_a_plan params.json

# Build by file path
upl build-from-json samples/create_rest_api.txt params.json

# Use the bundled sample inputs (one per sample prompt)
upl build-from-json samples/create_a_plan.txt samples_json_inputs/create_a_plan.json
```

The JSON file must be an object whose keys are parameter names (matched
case-insensitively). A `null` value for a key means "use the default".

```json
{
  "api_name": "Blog API",
  "language": "ruby",
  "resources": [
    {
      "name": "users",
      "actions": ["GET", "POST", "DELETE"],
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "email", "type": "string", "required": true }
      ]
    }
  ]
}
```

Type mapping:

| UPL type | JSON value |
|---|---|
| `string` / `long_string` | string |
| `number` | number |
| `boolean` | boolean |
| `object` | object (missing fields use defaults) |
| `list` | array (each element per the list's `etype`) |
| `option_single` | a single value matching the `etype` and one of `opts` |
| `option_multi` | array of values, each matching the `etype` and one of `opts` |

The rendered prompt can be piped directly to an LLM tool:

```bash
upl build-from-json create_rest_api params.json | aichat
```

If what you want is a voice response from llm, check [vtmate](https://www.github.com/DavidValin/vtmate), example:

```bash
upl > prompt.txt
vtmate -i prompt.txt
```

## Build History

UPL tracks every prompt build in `~/.upl/build_history.json`. After each field
is collected, the build record is persisted — so if you cancel or close the
terminal mid-build, you can resume from the exact point of interruption.

Each record stores a uuid, date, prompt sha256, status (`in_progress` or
`built`), and the collected field values.

### Resume and Rebuild

Press `Ctrl+H` to open the build-history sidebar:

- In the **prompt browser** — `Ctrl+H` opens the sidebar at any time.
- During a **build** — after submitting a field (pressing Enter), a brief
  window lets you press `Ctrl+H` to open the sidebar.

From the sidebar:

| Key           | Action                                                  |
|---------------|---------------------------------------------------------|
| ↑ / ↓         | navigate through build records                           |
| Enter         | resume (if in progress) or rebuild (if built)           |
| Ctrl+E        | export the selected record's values to a JSON file      |
| Ctrl+D        | delete the selected record                              |
| Esc / q       | close the sidebar                                       |

Resuming an in-progress build pre-fills the already-collected fields,
displays them for review, and continues from the next un-collected field.
Rebuilding a completed build pre-fills all fields, displays them, and
positions the cursor at the **last** field so you can review it and
immediately build (or press Esc to go back and edit earlier fields).

### Exporting a Build

Press `Ctrl+E` in the build-history sidebar to export the selected record's
collected parameter values as a pretty JSON file. The file is written to:

```
~/.upl/build_exports/<prompt_sha256>_<YYYYMMDD_HHMM>.json
```

The exported JSON contains only the parameter values (not the build metadata)
and is directly reusable with `upl build-from-json`:

```bash
upl build-from-json samples/create_a_plan.txt ~/.upl/build_exports/ab12..._20260803_1430.json
```

The repo also ships ready-made sample inputs in `./samples_json_inputs/` — one
pretty-printed JSON file per prompt in `./samples/`:

```bash
# try any sample with its matching JSON input
upl build-from-json samples/analyze_argument.txt samples_json_inputs/analyze_argument.json
upl build-from-json samples/teach_foundations.txt samples_json_inputs/teach_foundations.json
```

### Disabling History

Build history is enabled by default. To disable it for a session:

```bash
upl --no-history
upl build --no-history samples/create_a_plan.txt
# or: upl build -nh samples/create_a_plan.txt
```

When history is disabled, builds are not tracked and `Ctrl+H` is not available.

## Prompt Editor

UPL ships with a built-in terminal text editor for authoring and editing
prompts without leaving `upl`. It opens a full-screen, two-pane view: an
editable area on the left and a live list of the prompt's declared variables
on the right. The content is re-parsed on every keystroke, so a VALID /
INVALID badge in the status bar always reflects the current state.

Key bindings:

| Key           | Action                                                  |
|---------------|---------------------------------------------------------|
| arrows / PgUp / PgDn / Home / End | navigate               |
| type          | insert                                                  |
| Enter         | new line                                                |
| Backspace / Delete | erase                                              |
| Tab           | insert 2 spaces                                         |
| Ctrl+S        | save (only when VALID) to `~/.upl/prompts/<name>.txt`   |
| Ctrl+R        | open the UPL RFC reference popup                        |
| Esc / Ctrl+C  | quit back to the list                                   |

There are two ways to open the editor:

```bash
# Create a new prompt from the skeleton template.
upl init

# Edit an existing prompt from the browser.
upl            # then press 'e' on a prompt to edit it, or 'n' to create one
```

`upl init` drops you straight into the editor pre-filled with a minimal valid
UPL skeleton; rename it, add your params, write the body, and press Ctrl+S to
save it into `~/.upl/prompts/`. From the browser, pressing `n` does the same
and refreshes the list on save, while `e` opens the selected prompt for
editing.

### UPL Repository

For more details about upl reposity see [`REPOSITORY.md`](REPOSITORY.md)

## Creating a upl repository

You can create a upl repository simply by running:

```bash
upl start_repository
```

Add a user to the repository (from the server machine):

```bash
upl register_user
```

## Accessing a repository

You can `push` `pull` prompts from a repository:

```bash
# Configure the repository (once)
upl set-rep http://remote-machine
upl login
upl push my_nice_prompt.txt
upl pull a_user/my_nice_prompt
```

## CLI Commands

```
  init             Create a new UPL prompt from the skeleton in the editor.
  build (alias: b) Browse a prompt library or build a single prompt file.
  build-from-json   Build a prompt from a JSON file of parameter values (validated, non-interactive).
  login            Log in to the configured repository (stores a session token).
  push             Push a local prompt to the repository (uses its `name` as name).
  pull             Pull a prompt from the repository into ~/.upl/prompts.
  del              Delete all versions of one of your prompts from the repository.
  set-rep          Configure the repository endpoint (and optional TLS / GPG key).
  get-rep          Show the configured repository endpoint.
  start_repository Start the repository TCP/TLS server.
  register_user    Create a repository user (local admin, run on the server host).
  delete_user      Remove a repository user (local admin, run on the server host).

Options:
  --no-input        Skip interactive prompts and render using declared defaults.
  --no-history, -nh Disable build history tracking for this session.
```

## Run the tests

```bash
make test
```
