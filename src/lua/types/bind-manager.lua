---@meta _

---A binding manager.
---
---Its able to register and unregister keymaps and mouse maps from the main compositor state.
---@class _bind_manager
local bind_manager = {}

---Binds a given key pattern.
---
---Overrides any previous binds of this key with given modifiers.
---@param key_pattern _key_pattern|string # The key pattern to bind the callback onto
---@param callback fun(...:any): any # the callback to run when the keybind is pressed
---@param info {group:string, description:string} # The description/info of this key bind
function bind_manager:bind_key(key_pattern, callback, info) end

---Unbinds a given key pattern.
---@param key_pattern _key_pattern|string # The key pattern to unbind the callback from
function bind_manager:unbind_key(key_pattern) end

---Get a table with all the registered key binds.
---@return table<_key_pattern, _key_bind>
function bind_manager:key_binds() end

---A combo of modifiers and a single key.
---@class _key_pattern
---@field [1] _modifier[]|_modifier # The modifiers of this key pattern
---@field [2] string # The UTF-8 representation of the key of this pattern, described by XKB.
local key_pattern = {}

---@alias _modifier
---| "alt" # the alt/mod1 key.
---| "ctrl" # the control key.
---| "shift" # the shift key.
---| "super" # the super/log/windows key.

---A key bind, with its group, description and callback.
---@class _key_bind
---@field group string|nil # The (optional) group name of this keybind.
---@field description string|nil # The (optional) description of this key bind.
local key_bind = {}

---Run the callback bound to this key bind.
function key_bind:run_callback() end
