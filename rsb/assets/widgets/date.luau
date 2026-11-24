--!strict
-- The date. A date format has no hours or minutes, so it ticks once an hour.
local rsb = require("@rsb")

return rsb.widget {
	options = {
		format = "ddd d MMM",
		timezone = "local",
	},
	permissions = {},
	setup = function(props, ctx)
		local now = rsb.clock(rsb.time.precision(props.format), props.timezone)
		return rsb.row {
			class = "item date",
			rsb.text {
				class = "value",
				text = function(): string
					return rsb.time.format(now.value, props.format)
				end,
			},
		}
	end,
}
