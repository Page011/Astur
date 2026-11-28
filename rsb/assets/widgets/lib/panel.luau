--!strict
-- Pieces the panel and popup widgets share: reading a service's state, one-line
-- notes, icon buttons, the audio device rows and a few time helpers.
local rsb = require("@rsb")

local M = {}

-- A service that is not simply giving values: "stale" (the last reading is
-- too old), "error", "unavailable", "starting" (nothing read yet), or "ok".
function M.state_of(svc: any): string
	if svc.stale then
		return "stale"
	end
	local status = svc.status :: string
	if status == "error" then
		return "error"
	elseif status == "unavailable" then
		return "unavailable"
	elseif status == "starting" or status == "off" then
		return "starting"
	end
	return "ok"
end

-- One short line in place of a list.
function M.note(text: any): any
	return rsb.text { class = "note", text = text }
end

-- A square button with one icon; `class` adds to "icon-btn".
function M.icon_button(icon: any, tooltip: string, on_click: () -> (), class: any?, autofocus: boolean?): any
	return rsb.row {
		class = if class ~= nil then class else "icon-btn",
		focusable = true,
		autofocus = autofocus,
		tooltip = tooltip,
		rsb.icon { svg = icon },
		on_click = on_click,
	}
end

-- The service's own words for why it gives no list, or nil while it does.
-- `reading` is how `what` reads in the middle of a sentence (default: as given).
function M.service_note(svc: any, what: string, reading: string?): string?
	local state = M.state_of(svc)
	if state == "stale" then
		return what .. " is out of date"
	elseif state == "error" then
		return what .. " is not working"
	elseif state == "unavailable" then
		return what .. " is not available"
	elseif state == "starting" then
		return "Reading " .. (reading or what) .. "..."
	end
	return nil
end

-- Milliseconds since 1970 for a UTC time (days from civil date: Howard
-- Hinnant's algorithm), to age a position a player reported.
function M.epoch_ms(t: { year: number, month: number, day: number, hour: number, minute: number, second: number, millis: number }): number
	local y = if t.month <= 2 then t.year - 1 else t.year
	local era = math.floor(y / 400)
	local yoe = y - era * 400
	local mp = (t.month + 9) % 12
	local doy = math.floor((153 * mp + 2) / 5) + t.day - 1
	local doe = yoe * 365 + math.floor(yoe / 4) - math.floor(yoe / 100) + doy
	local days = era * 146097 + doe - 719468
	return ((days * 24 + t.hour) * 60 + t.minute) * 60000 + t.second * 1000 + t.millis
end

-- Days since 1970-01-01 for a calendar date.
function M.days_from_civil(year: number, month: number, day: number): number
	local y = if month <= 2 then year - 1 else year
	local era = math.floor(y / 400)
	local yoe = y - era * 400
	local mp = (month + 9) % 12
	local doy = math.floor((153 * mp + 2) / 5) + day - 1
	local doe = yoe * 365 + math.floor(yoe / 4) - math.floor(yoe / 100) + doy
	return era * 146097 + doe - 719468
end

-- The player's position in seconds now, aged by the time since the player
-- last reported it while a track plays; nil when it reports none.
function M.media_position(media: any): number?
	local position = media.position :: number?
	if media.stale or position == nil then
		return nil
	end
	local at = media.position_at :: number?
	if media.playing == true and at ~= nil then
		position += math.max(0, M.epoch_ms(rsb.time.now("utc")) - at) / 1000
	end
	return position
end

-- A row that is a volume slider: the slider is the row's whole background (its
-- fill shows the level), and the parts come after it drawn over it. A slider
-- cannot hold children, and text over it must not catch the pointer, so the
-- stylesheet gives the parts negative margins and `pointer-events: none`.
function M.slider_row(
	class: string,
	value: any,
	on_change: (number, boolean) -> (),
	disabled: any?,
	parts: { any },
	click: (() -> ())?,
	tooltip: string?
): any
	-- With a click handler a press on the row that does not drag is a click and
	-- leaves the level alone; a drag still sets it.
	local row: any = {
		class = class,
		tooltip = tooltip,
		rsb.slider {
			class = "track",
			value = value,
			min = 0,
			max = 100,
			step = 1,
			jump = click == nil,
			disabled = disabled,
			on_change = on_change,
			on_click = click,
		},
	}
	for _, part in parts do
		table.insert(row, part)
	end
	return rsb.box(row)
end

-- One audio device: drag along the row to set that device's volume, click it
-- or press the leading button to make it the default output, the trailing
-- button mutes it. The default device is the current row.
function M.device_row(audio: any, dev: any): any
	local id = dev.id :: string
	local function use()
		if not dev.default then
			audio:set_default(id)
		end
	end
	return M.slider_row(
		if dev.default then "row output current" else "row output",
		dev.volume,
		function(value: number, done: boolean)
			audio:set_device_volume(id, value)
		end,
		nil,
		{
			M.icon_button(
				if dev.bluetooth then "headphones" else "speaker",
				if dev.default then "Current output" else "Use this output",
				use,
				"icon-btn lead"
			),
			rsb.text { class = "name ellipsis", text = dev.name },
			rsb.text { class = "level", text = string.format("%d%%", math.floor((dev.volume :: number) + 0.5)) },
			M.icon_button(
				if dev.muted then "volume-mute" else "volume",
				if dev.muted then "Unmute" else "Mute",
				function()
					audio:set_device_muted(id, not dev.muted)
				end,
				"icon-btn mute"
			),
		},
		use,
		dev.name
	)
end

-- The rows for every output device, the default first (the service sorts
-- them that way); nothing while the list is out of date.
function M.device_rows(audio: any): () -> { any }
	return rsb.each(function(): { any }
		if audio.stale then
			return {}
		end
		return (audio.devices :: { any }?) or {}
	end, function(dev: any): any
		return M.device_row(audio, dev)
	end, function(dev: any): string
		return dev.id
	end)
end

return M
