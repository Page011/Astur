--!strict
-- Time of day. Clicking swaps to alt_format (set it to "" to turn that off).
local rsb = require("@rsb")

return rsb.widget {
	options = {
		format = "HH:mm",
		alt_format = "ddd d MMM HH:mm:ss",
		timezone = "local",
	},
	permissions = {},
	setup = function(props, ctx)
		local zone = props.timezone
		local alt = rsb.state(false)
		local now = rsb.state(rsb.time.now(zone))

		-- The clock ticks at the precision of the format on show: a minute
		-- clock wakes once a minute, and a seconds format only costs a tick
		-- a second while it is toggled on.
		rsb.effect(function()
			local format = if alt.value then props.alt_format else props.format
			local tick = rsb.clock(rsb.time.precision(format), zone)
			rsb.effect(function()
				now.value = tick.value
			end)
		end)

		local label = rsb.text {
			class = "value",
			text = function(): string
				return rsb.time.format(now.value, if alt.value then props.alt_format else props.format)
			end,
		}

		if props.alt_format == "" then
			return rsb.row { class = "item clock", label }
		end
		return rsb.row {
			class = "item clock button",
			label,
			on_click = function()
				alt.value = not alt.value
			end,
		}
	end,
}
