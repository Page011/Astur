--!strict
-- The collapsed tray: an empty segment the width of a sliver. It never grows
-- while it holds no icons, and RSB has no tray icons yet, so for now it only
-- keeps the shape of the right-hand group. Class: item, tray.
local rsb = require("@rsb")

return rsb.widget {
	about = { title = "Tray" },
	permissions = {},
	setup = function(props, ctx)
		return rsb.row { class = "item tray" }
	end,
}
