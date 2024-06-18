---@meta _

---@class _fht
fht = {}

---Log an info message to stdout, via tracing.
---@param msg string # the logged message
function fht.info(msg) end

---Log a warn message to stdout, via tracing.
---@param msg string # the logged message
function fht.warn(msg) end

---Log an error message to stdout, via tracing.
---@param msg string # the logged message
function fht.error(msg) end

---Log a debug message to stdout, via tracing.
---@param msg string # the logged message
function fht.debug(msg) end

---Register a new callback for a given signal.
---@param signal _fht_signal # the signal you want to register callback onto
---@param callback fun(...: any) # the callback to execute
---@return integer # the id of this callback.
function fht.register_callback(signal, callback) end

---Unregister a callback for a given signal.
---@param signal _fht_signal # the signal you want to register callback onto
---@param callback_id integer # the callback to remove.
function fht.unregister_callback(signal, callback_id) end

---@alias _fht_signal
---| "test" # A test signal to see if the lua virtual machine receives signals or not.
