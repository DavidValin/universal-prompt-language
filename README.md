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
5. UPL `repository server`, to create a centralized repository of upl prompts
6. UPL `repository client`, to publish/download upl prompts to/from a an upl repository

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

If what you want is a voice response from llm, check [vtmate](https://www.github.com/DavidValin/vtmate), example:

```bash
upl > prompt.txt
vtmate -i prompt.txt
```

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
  login            Log in to the configured repository (stores a session token).
  push             Push a local prompt to the repository (uses its `name` as name).
  pull             Pull a prompt from the repository into ~/.upl/prompts.
  del              Delete all versions of one of your prompts from the repository.
  set-rep          Configure the repository endpoint (and optional TLS / GPG key).
  get-rep          Show the configured repository endpoint.
  start_repository Start the repository TCP/TLS server.
  register_user    Create a repository user (local admin, run on the server host).
  delete_user      Remove a repository user (local admin, run on the server host).
```

## Run the tests

```bash
make test
```
