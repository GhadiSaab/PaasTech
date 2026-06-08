# PaasTech CLI

`paastech` - deploy and manage your applications from the terminal.

## Shell completion

The CLI supports two completion modes:

- **Dynamic (recommended)**: application and resource names are suggested by querying the API at completion time.
- **Static**: a script generated once, with no network call. App/resource names are not completed.

### Bash

**Dynamic** - run once (re-run after CLI updates):

```bash
COMPLETE=bash paastech > ~/.local/share/bash-completion/completions/paastech
```

This saves the completion script to disk. Bash loads it lazily (only on the first Tab press for `paastech`), and the script queries the API at completion time - the binary is never called at shell startup.

**Static**:

```bash
paastech completion bash > ~/.local/share/bash-completion/completions/paastech
```

> Reload the shell afterwards: `source ~/.bashrc`

---

### Zsh

**Dynamic** - add to `~/.zshrc`:

```zsh
source <(COMPLETE=zsh paastech)
```

**Static**:

```zsh
paastech completion zsh > "${fpath[1]}/_paastech"
```

> Reload the shell afterwards: `source ~/.zshrc`

---

### Fish

**Dynamic** - add to `~/.config/fish/config.fish`:

```fish
COMPLETE=fish paastech | source
```

**Static**:

```fish
paastech completion fish > ~/.config/fish/completions/paastech.fish
```

---

### PowerShell

**Static**:

```powershell
paastech completion powershell | Out-String | Invoke-Expression
```

To persist, add the line above to your `$PROFILE`.
