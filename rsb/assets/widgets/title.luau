--!strict
-- The title of the focused window, cut with an ellipsis at `max_width`.
-- Empty on the desktop. With `show_process` the program name comes first.
local rsb = require("@rsb")

local function program(file: string): string
	local name = string.gsub(file, "%.[eE][xX][eE]$", "")
	return name
end

return rsb.widget {
	options = {
		max_width = 360,
		show_process = false,
	},
	permissions = { read = { "window" } },
	setup = function(props, ctx)
		local window = rsb.services.window

		return rsb.row {
			-- Nothing on the desktop, and nothing before the first reading.
			visible = function(): boolean
				return window.stale or window.has_window == true
			end,
			class = function(): string
				return if window.stale then "item title stale" else "item title"
			end,
			rsb.text {
				class = "ellipsis",
				style = { max_width = props.max_width },
				text = function(): string
					if window.stale then
						return "--"
					end
					local title = (window.title :: string?) or ""
					if props.show_process then
						local file = (window.process :: string?) or ""
						return if title == "" then program(file) else program(file) .. " - " .. title
					end
					return title
				end,
			},
		}
	end,
}
