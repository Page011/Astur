--!strict
-- Time of day. Clicking swaps to alt_format (set it to "" to turn that off).
-- hover_format is what the clock shows while the pointer is over it ("" for
-- nothing): a seconds format here costs a tick a second only while hovered.
local rsb = require("@rsb")

return rsb.widget {
	about = { title = "Clock" },
	options = {
		format = "HH:mm",
		alt_format = "ddd d MMM HH:mm:ss",
		hover_format = "",
		timezone = "local",
	},
	permissions = {},
	setup = function(props, ctx)
		local zone = props.timezone
		local alt = rsb.state(false)
		local hovered = rsb.state(false)
		local now = rsb.state(rsb.time.now(zone))

		local function shown_format(): string
			if hovered.value and props.hover_format ~= "" then
				return props.hover_format
			end
			return if alt.value then props.alt_format else props.format
		end

		-- The clock ticks at the precision of the format on show: a minute
		-- clock wakes once a minute, and a seconds format only costs a tick
		-- a second while it is toggled on or hovered.
		rsb.effect(function()
			local tick = rsb.clock(rsb.time.precision(shown_format()), zone)
			now.value = rsb.time.now(zone)
			rsb.effect(function()
				now.value = tick.value
			end)
		end)

		local label = rsb.text {
			class = "value",
			text = function(): string
				return rsb.time.format(now.value, shown_format())
			end,
		}

		local acts = props.alt_format ~= "" or props.hover_format ~= ""
		return rsb.row {
			class = if acts then "item clock button" else "item clock",
			label,
			on_click = if props.alt_format ~= ""
				then function()
					alt.value = not alt.value
				end
				else nil,
			on_enter = if props.hover_format ~= ""
				then function()
					hovered.value = true
				end
				else nil,
			on_leave = if props.hover_format ~= ""
				then function()
					hovered.value = false
				end
				else nil,
		}
	end,
}
