--!strict
-- A round button that starts a screen recording. With no `command` it opens
-- Windows' own screen recorder (Snipping Tool in video mode); set `command`
-- in bar.toml to run your own program instead, a list such as
-- ["obs64", "--startrecording"] or a string split by the Windows argument
-- rules, with no shell. Class: item, record, button.
local rsb = require("@rsb")
local cmdline = require("./lib/cmdline")

local RECORDER = "ms-screenclip://capture/video?enabledModes=RecordAllModes&user-agent=rsb"

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
	about = { title = "Record" },
	options = {
		command = (nil :: any),
	},
	optional = { "command" },
	permissions = { control = { "apps" }, exec = { "$option:command" } },
	setup = function(props, ctx)
		local command = argv(props.command)
		return rsb.row {
			class = "item record button",
			tooltip = "Record the screen",
			rsb.icon { svg = "video" },
			on_click = function()
				if command ~= nil then
					rsb.exec.run(command, { timeout = "20s", max_output = 4096 })
				else
					rsb.services.apps:open_uri(RECORDER)
				end
			end,
		}
	end,
}
