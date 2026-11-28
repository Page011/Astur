--!strict
-- Memory in use. Format fields: {value} percent, {used} and {total} in bytes
-- shown as KB / MB / GB. Shows -- while the reading is stale. `icons` is
-- "font" (the icon font) or "svg" (the bundled SVG icons; `icon` then names
-- one of them).
local rsb = require("@rsb")
local text = require("./lib/format")

return rsb.widget {
	about = { title = "Memory" },
	options = {
		format = "{value}%",
		icon = "memory",
		label = "",
		icons = "font",
		every = "2s",
		warn = 85,
	},
	permissions = {},
	setup = function(props, ctx)
		local system = rsb.services.system
		system:interval(props.every)

		local function used(): number?
			if system.stale then
				return nil
			end
			return system.memory :: number?
		end

		local lead = if props.label ~= ""
			then rsb.text { class = "label", text = props.label }
			elseif props.icons == "svg" then rsb.icon { svg = props.icon }
			else rsb.icon { name = props.icon :: any }

		return rsb.row {
			class = function(): string
				local v = used()
				if system.stale then
					return "item memory stale"
				elseif v ~= nil and v >= props.warn then
					return "item memory warn"
				end
				return "item memory"
			end,
			lead,
			rsb.text {
				class = "value",
				text = function(): string
					local v = used()
					if v == nil then
						return "--"
					end
					return text.apply(props.format, {
						value = text.round(v),
						used = rsb.format.bytes(system.memory_used),
						total = rsb.format.bytes(system.memory_total),
					})
				end,
			},
		}
	end,
}
