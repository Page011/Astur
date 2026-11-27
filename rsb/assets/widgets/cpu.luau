--!strict
-- Processor load. Shows -- while the reading is stale; gets class "warn"
-- from `warn` percent up. `icons` is "font" (the icon font) or "svg" (the
-- bundled SVG icons; `icon` then names one of them).
local rsb = require("@rsb")
local text = require("./lib/format")

return rsb.widget {
	about = { title = "CPU" },
	options = {
		format = "{value}%",
		icon = "cpu",
		label = "",
		icons = "font",
		every = "2s",
		warn = 80,
	},
	permissions = {},
	setup = function(props, ctx)
		local system = rsb.services.system
		system:interval(props.every)

		-- nil until the first sample arrives, and while the sample is stale.
		local function reading(): number?
			if system.stale then
				return nil
			end
			return system.cpu :: number?
		end

		local lead = if props.label ~= ""
			then rsb.text { class = "label", text = props.label }
			elseif props.icons == "svg" then rsb.icon { svg = props.icon }
			else rsb.icon { name = props.icon :: any }

		return rsb.row {
			class = function(): string
				local v = reading()
				if system.stale then
					return "item cpu stale"
				elseif v ~= nil and v >= props.warn then
					return "item cpu warn"
				end
				return "item cpu"
			end,
			lead,
			rsb.text {
				class = "value",
				text = function(): string
					local v = reading()
					if v == nil then
						return "--"
					end
					return text.apply(props.format, { value = text.round(v) })
				end,
			},
		}
	end,
}
