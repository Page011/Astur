--!strict
-- The date. A date format has no hours or minutes, so it ticks once an hour.
-- hover_format is what the date shows while the pointer is over it ("" for
-- nothing), for example "dddd d MMMM, yyyy" for the long form.
-- popup = true makes a click open the calendar (a popup called "calendar"):
-- the month with chevrons, the wheel or the arrow keys change month, today is
-- filled with the accent colour. Weeks start on Monday.
local rsb = require("@rsb")
local panel = require("./lib/panel")

local MONTHS = {
	"January",
	"February",
	"March",
	"April",
	"May",
	"June",
	"July",
	"August",
	"September",
	"October",
	"November",
	"December",
}
local WEEKDAYS = { "Mo", "Tu", "We", "Th", "Fr", "Sa", "Su" }

local function days_in_month(year: number, month: number): number
	local next_year = if month == 12 then year + 1 else year
	local next_month = if month == 12 then 1 else month + 1
	return panel.days_from_civil(next_year, next_month, 1) - panel.days_from_civil(year, month, 1)
end

-- The calendar's content: a header with the month, the weekday row and six
-- weeks of seven days, the days of the neighbouring months dimmed.
local function calendar(keys: { fn: ((any) -> ())? }): { any }
	local today = rsb.clock("hour")
	local now = rsb.time.now()
	local view = rsb.state({ year = now.year, month = now.month })

	local function shift(by: number)
		local v = view:peek()
		local month = v.month + by
		local year = v.year
		while month < 1 do
			month += 12
			year -= 1
		end
		while month > 12 do
			month -= 12
			year += 1
		end
		view.value = { year = year, month = month }
	end

	local function week(first_day_number: number, v: { year: number, month: number }, today_number: number, w: number): any
		local cells: any = { class = "week" }
		local here = days_in_month(v.year, v.month)
		local prev_year = if v.month == 1 then v.year - 1 else v.year
		local prev_month = if v.month == 1 then 12 else v.month - 1
		local before = days_in_month(prev_year, prev_month)
		local offset = (first_day_number + 4 + 6) % 7
		for c = 0, 6 do
			local n = w * 7 + c - offset + 1
			local number = first_day_number + (n - 1)
			local label = if n < 1 then before + n elseif n > here then n - here else n
			local class = "day"
			if n < 1 or n > here then
				class ..= " other"
			end
			if number == today_number then
				class ..= " today"
			end
			table.insert(cells, rsb.row { class = class, rsb.text { text = tostring(label) } })
		end
		return rsb.row(cells)
	end

	local weekdays: any = { class = "weekdays" }
	for _, name in WEEKDAYS do
		table.insert(weekdays, rsb.row { class = "weekday", rsb.text { text = name } })
	end

	keys.fn = function(e: any)
		local key = e.key :: string
		if key == "ArrowLeft" or key == "ArrowUp" or key == "PageUp" then
			shift(-1)
		elseif key == "ArrowRight" or key == "ArrowDown" or key == "PageDown" then
			shift(1)
		elseif key == "Home" then
			local t = rsb.time.now()
			view.value = { year = t.year, month = t.month }
		end
	end
	rsb.on_cleanup(function()
		keys.fn = nil
	end)

	return {
		rsb.column {
			class = "calendar-body",
			on_wheel = function(e)
				shift(if e.delta > 0 then -1 else 1)
			end,
			rsb.row {
				class = "card head",
				panel.icon_button("chevron-left", "Previous month", function()
					shift(-1)
				end),
				rsb.text {
					class = "title",
					text = function(): string
						local v = view.value
						return MONTHS[v.month] .. " " .. tostring(v.year)
					end,
				},
				panel.icon_button("chevron-right", "Next month", function()
					shift(1)
				end),
			},
			rsb.row(weekdays),
			rsb.column {
				class = "weeks",
				function(): { any }
					local v = view.value
					local t = today.value
					local first = panel.days_from_civil(v.year, v.month, 1)
					local today_number = panel.days_from_civil(t.year, t.month, t.day)
					local out: { any } = {}
					for w = 0, 5 do
						table.insert(out, week(first, v, today_number, w))
					end
					return out
				end,
			},
		},
	}
end

return rsb.widget {
	about = { title = "Date", opens = { "calendar" } },
	options = {
		format = "ddd d MMM",
		hover_format = "",
		timezone = "local",
		popup = false,
	},
	permissions = {},
	setup = function(props, ctx)
		local hovered = rsb.state(false)
		local shown = rsb.state(false)
		local keys: { fn: ((any) -> ())? } = { fn = nil }
		local now = rsb.clock(
			rsb.time.precision(if props.hover_format ~= "" then props.hover_format else props.format),
			props.timezone
		)
		local acts = props.hover_format ~= ""
		local row: any = {
			class = if acts or props.popup then "item date button" else "item date",
			rsb.text {
				class = "value",
				text = function(): string
					local format = if hovered.value and acts then props.hover_format else props.format
					return rsb.time.format(now.value, format)
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
		if props.popup then
			row.on_click = function()
				shown.value = not shown.value
			end
			table.insert(
				row,
				rsb.popup {
					open = shown,
					name = "calendar",
					class = "panel calendar",
					placement = "below",
					align = "center",
					on_key = function(e)
						local handler = keys.fn
						if handler ~= nil then
							handler(e)
						end
					end,
					function(): { any }
						return calendar(keys)
					end,
				}
			)
		end
		return rsb.row(row)
	end,
}
