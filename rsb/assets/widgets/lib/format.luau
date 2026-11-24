--!strict
-- Text helpers shared by the built-in widgets.

export type Values = { [string]: string | number }

local M = {}

-- Replaces {name} in the template with values[name]. A name with no value is
-- left as written, so a typo in a format string shows up on the bar.
function M.apply(template: string, values: Values): string
	local out = string.gsub(template, "{([%w_]+)}", function(name: string): string?
		local v = values[name]
		if v == nil then
			return nil
		end
		return tostring(v)
	end)
	return out
end

-- True when the template mentions {name}.
function M.uses(template: string, name: string): boolean
	return string.find(template, "{" .. name .. "}", 1, true) ~= nil
end

-- A percentage as a whole number.
function M.round(n: number): number
	return math.floor(n + 0.5)
end

-- Trims spaces and a dangling " - " left behind when one half of a
-- "{artist} - {title}" format is empty.
function M.tidy(text: string): string
	local s = string.gsub(text, "^%s*%-%s+", "")
	s = string.gsub(s, "%s+%-%s*$", "")
	s = string.gsub(s, "^%s+", "")
	s = string.gsub(s, "%s+$", "")
	return s
end

-- The first line of some output, without the line ending.
function M.first_line(text: string): string
	local line = string.match(text, "^[^\r\n]*")
	return line or ""
end

return M
