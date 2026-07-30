# Universal Prompt Language

UPL is a language to define dynamic prompts which contain variables, conditionals and loops. This allows you to dynamically construct complex prompts, [see the language specification](upl-spec/upl-1.0-rfc.pdf). UPL files are just `.txt` or `.upl` files, see examples at `samples/` directory.

![preview](https://github.com/DavidValin/upl/raw/master/preview.gif)

This project contains:

1. `upl specification`
2. terminal `prompts browser` with tag support
3. terminal `prompts builder`, to construct a dynamic prompt interactively by filling in the input variables
4. terminal `prompt editor` with syntax highlight
5. UPL `repository server`, to create a centralized repository of upl prompts
6. UPL `repository client`, to publish/download upl prompts to/from a an upl repository

All operations are integrated in a single cli command called `upl`.

### Quickview

* Specification: [`upl-spec/upl-1.0-rfc.pdf`](upl-spec/upl-1.0-rfc.pdf)
* Prompt Browser: `upl <prompts_folder>` (or `upl` alone to browse `~/.upl`)
* Prompt Builder: `upl b <prompt_file.txt>` (builds a single file directly)
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
  `review_article.txt`, `teach_foundations.txt`).
- `~/.upl/tags_db` — a pre-built tag store associating those sample prompts
  with their tags (e.g. `software development`, `planning`, `understand`,
  `analysis`).

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

## Build a prompt from an upl prompt

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
