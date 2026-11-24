--!strict
-- A widget you make from bar.toml alone: fixed text, or the first line a
-- command prints, refreshed every `every`.
--   text    fixed text
--   exec    program to run: a list (["git", "status", "--short"]) or a
--           string split by the Windows argument rules, with no shell. For
--           pipes and other shell features write
--           exec = ["cmd", "/d", "/c", "your command"]; that gives the widget
--           the right to run cmd, which can run anything.
--   every   how often exec runs
--   format  how the output shows: {output} and {text}
--   click   program to run on a click, same forms as exec
--   class   one more class on the widget, for style.css
-- A failed command shows -- (class "stale"); nothing to show hides the widget.
local rsb = require("@rsb")
local text = require("./lib/format")
local cmdline = require("./lib/cmdline")

-- A program and its arguments from either form of the option.
local function argv(value: any): { string }?
	if type(value) == "string" then
		local words = cmdline.split(value)
		return if #words > 0 then words else nil
	elseif type(value) == "table" and #value > 0 then
		return value
	end
	return nil
end

return rsb.widget {
	options = {
		text = "",
		exec = (nil :: any),
		every = "30s",
		format = "{output}",
		click = (nil :: any),
		class = "",
	},
	optional = { "exec", "click" },
	permissions = { exec = { "$option:exec", "$option:click" } },
	setup = function(props, ctx)
		local command = argv(props.exec)
		local on_click_command = argv(props.click)

		-- nil: no answer yet. Otherwise the output, or false when it failed.
		local output = rsb.state(nil :: string | boolean | nil)

		local function refresh()
			if command == nil then
				return
			end
			local result = rsb.exec.run(command, { timeout = "20s", max_output = 65536 })
			if result.ok then
				output.value = text.first_line(result.stdout)
			else
				output.value = false
			end
		end

		if command ~= nil then
			rsb.timer.every(props.every, refresh)
			rsb.task.spawn(refresh)
		end

		local function shown(): string?
			if command == nil then
				return if props.text ~= "" then props.text else nil
			end
			local o = output.value
			if o == nil or o == false then
				return "--"
			end
			return text.apply(props.format, { output = o :: string, text = props.text })
		end

		local extra = if props.class ~= "" then " " .. props.class else ""
		local base = if on_click_command ~= nil then "item text button" else "item text"

		return rsb.row {
			-- Nothing to show: no space either.
			visible = function(): boolean
				local s = shown()
				return s ~= nil and s ~= ""
			end,
			class = function(): string
				return if output.value == false then base .. " stale" .. extra else base .. extra
			end,
			rsb.text {
				class = "value",
				text = function(): string
					return shown() or ""
				end,
			},
			on_click = if on_click_command ~= nil
				then function()
					rsb.exec.run(on_click_command, { timeout = "20s", max_output = 4096 })
					refresh()
				end
				else nil,
		}
	end,
}
