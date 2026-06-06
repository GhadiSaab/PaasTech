# CLI

## Installation

```bash
  cd cli/
  cargo install --path .
```

## Shell completion ZSH (one-time)

```bash
  paastech completion zsh > ~/.zfunc/_paastech
  source ~/.zshrc
```

## Shell completion Bash (one-time)

```bash
  mkdir -p ~/.local/share/bash-completion/completions
  paastech completion bash > ~/.local/share/bash-completion/completions/paastech
```

## Shell completion Fish (one-time)

```bash
  paastech completion fish > ~/.config/fish/completions/paastech.fish
```