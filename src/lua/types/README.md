# fht-compositor lua type annotations.

To help ease writing configuration and scripts, you can include this directory in your LSP settings
to get auto-completion (assuming you are using
[lua-language-server](https://github.com/LuaLS/lua-language-server) for your lua LSP)

- Neovim instructions:

```lua
require("lspconfig").lua_ls.setup {
    -- handlers and capabilities, up to you.
    settings = {
        Lua = {
            workspace = {
                library = {
                    -- other stuff, up to you...
                    ["/path/to/lua/types"] = true,
                }
            }
        }
    }
}
```
