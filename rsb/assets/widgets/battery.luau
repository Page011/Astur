--!strict
-- Battery level. A PC without a battery shows a plug icon (class "absent"),
-- or text, or nothing, as `absent` says: "icon", "text" or "hide".
-- Classes: charging, low (15% or less on battery), saver, absent, stale.
--   format        the text, {value} is the percent and {time} the time left
--   hover_format  the text while the pointer is over the widget ("" for
--                 nothing), for example "{time}"
--   unknown       what {time} shows when Windows reports no time left, as
--                 when charging or full
--   icons         "font" (the icon font, the default) or "svg" (the bundled
--                 SVG icons)
local rsb = require("@rsb")
local text = require("./lib/format")

-- Seconds as h:mm, like 3:05.
local function clock_time(seconds: number): string
	local minutes = math.floor(seconds / 60 + 0.5)
	return string.format("%d:%02d", math.floor(minutes / 60), minutes % 60)
end

return rsb.widget {
	options = {
		format = "{value}%",
		hover_format = "",
		unknown = "--:--",
		label = "",
		absent = "icon",
		icons = "font",
	},
	permissions = {},
	setup = function(props, ctx)
		local battery = rsb.services.battery
		local hovered = rsb.state(false)
		local svg = props.icons == "svg"

		-- Which glyph to draw: the charging bolt, the saver leaf, or a
		-- level from 0 to 10 in steps of ten percent (0 to 4 in steps of
		-- twenty-five for the SVG set). The plug is for a PC with no
		-- battery; a stale reading keeps the last level (the class `stale`
		-- mutes it), so it is not mistaken for mains power.
		local function glyph(): string
			local percent = (battery.percent :: number?) or 0
			if svg then
				if battery.present ~= true then
					return "plug"
				elseif battery.charging then
					return "battery-charging"
				end
				return "battery-" .. math.clamp(math.floor(percent / 25), 0, 4)
			end
			if battery.present ~= true then
				return "battery_none"
			elseif battery.charging then
				return "battery_charging"
			elseif battery.saver then
				return "battery_saver"
			end
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
			elseif svg then rsb.icon { svg = glyph }
			else rsb.icon { name = glyph :: any }

		local acts = props.hover_format ~= ""

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
				local base = if acts then "item battery button" else "item battery"
				return if s == "" then base else base .. " " .. s
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
					local seconds = battery.seconds_left :: number?
					return text.apply(if hovered.value and acts then props.hover_format else props.format, {
						value = text.round(percent),
						time = if seconds ~= nil and seconds > 0 then clock_time(seconds) else props.unknown,
					})
				end,
			},
			on_enter = if acts
				then function()
					hovered.value = true
				end
				else nil,
			on_leave = if acts
				then function()
					hovered.value = false
				end
				else nil,
		}
	end,
}
