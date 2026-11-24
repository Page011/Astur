--!strict
-- Battery level. A PC without a battery shows a plug icon (class "absent"),
-- or text, or nothing, as `absent` says: "icon", "text" or "hide".
-- Classes: charging, low (15% or less on battery), saver, absent, stale.
local rsb = require("@rsb")
local text = require("./lib/format")

return rsb.widget {
	options = {
		format = "{value}%",
		label = "",
		absent = "icon",
	},
	permissions = {},
	setup = function(props, ctx)
		local battery = rsb.services.battery

		-- Which glyph to draw: the charging bolt, the saver leaf, or a
		-- level from 0 to 10 in steps of ten percent. The plug is for a PC
		-- with no battery; a stale reading keeps the last level (the class
		-- `stale` mutes it), so it is not mistaken for mains power.
		local function glyph(): string
			if battery.present ~= true then
				return "battery_none"
			elseif battery.charging then
				return "battery_charging"
			elseif battery.saver then
				return "battery_saver"
			end
			local percent = (battery.percent :: number?) or 0
			return "battery_" .. math.clamp(math.floor(percent / 10), 0, 10)
		end

		local function state(): string
			if battery.stale then
				return "stale"
			elseif battery.present == nil then
				return ""
			elseif battery.present ~= true then
				return "absent"
			elseif battery.charging then
				return "charging"
			end
			local percent = (battery.percent :: number?) or 100
			if percent <= 15 and not battery.plugged then
				return "low"
			end
			return if battery.saver then "saver" else ""
		end

		local lead = if props.label ~= ""
			then rsb.text { class = "label", text = props.label }
			else rsb.icon { name = glyph :: any }

		return rsb.row {
			-- Nothing to show before the first reading, and nothing for a
			-- missing battery when `absent` is "hide".
			visible = function(): boolean
				if battery.stale then
					return true
				end
				local present = battery.present :: boolean?
				if present == nil then
					return false
				end
				return present or props.absent ~= "hide"
			end,
			class = function(): string
				local s = state()
				return if s == "" then "item battery" else "item battery " .. s
			end,
			lead,
			rsb.text {
				class = "value",
				text = function(): string
					if battery.stale or battery.present == nil then
						return "--"
					elseif battery.present ~= true then
						return if props.absent == "text" then "AC" else ""
					end
					local percent = battery.percent :: number?
					if percent == nil then
						return "--"
					end
					return text.apply(props.format, { value = text.round(percent) })
				end,
			},
		}
	end,
}
