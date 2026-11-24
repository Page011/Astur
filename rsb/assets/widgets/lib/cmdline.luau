--!strict
-- Splits a command line into words by the Windows argument rules (the C
-- runtime's), the same rules rsb applies to a string in `permissions.exec`
-- through "$option:name": whitespace separates words, a double quote toggles
-- quoting, a backslash is literal unless it comes right before a quote, and
-- the first word is a program name, so its backslashes are always literal.
-- Nothing a shell would interpret (|, &, %VAR%) is treated specially.

local M = {}

local function is_space(c: string): boolean
	return c == " " or c == "\t"
end

function M.split(line: string): { string }
	local words: { string } = {}
	local n = #line
	local i = 1

	while i <= n and is_space(string.sub(line, i, i)) do
		i += 1
	end
	if i <= n then
		local w: { string } = {}
		if string.sub(line, i, i) == '"' then
			i += 1
			while i <= n and string.sub(line, i, i) ~= '"' do
				table.insert(w, string.sub(line, i, i))
				i += 1
			end
			i += 1
		end
		while i <= n and not is_space(string.sub(line, i, i)) do
			local c = string.sub(line, i, i)
			if c ~= '"' then
				table.insert(w, c)
			end
			i += 1
		end
		table.insert(words, table.concat(w))
	end

	while true do
		while i <= n and is_space(string.sub(line, i, i)) do
			i += 1
		end
		if i > n then
			break
		end
		local w: { string } = {}
		local quoted = false
		while i <= n and (quoted or not is_space(string.sub(line, i, i))) do
			local c = string.sub(line, i, i)
			if c == "\\" then
				local start = i
				while i <= n and string.sub(line, i, i) == "\\" do
					i += 1
				end
				local count = i - start
				if i <= n and string.sub(line, i, i) == '"' then
					table.insert(w, string.rep("\\", count // 2))
					if count % 2 == 1 then
						table.insert(w, '"')
						i += 1
					end
				else
					table.insert(w, string.rep("\\", count))
				end
			elseif c == '"' then
				if quoted and string.sub(line, i + 1, i + 1) == '"' then
					table.insert(w, '"')
					i += 2
				else
					quoted = not quoted
					i += 1
				end
			else
				table.insert(w, c)
				i += 1
			end
		end
		table.insert(words, table.concat(w))
	end
	return words
end

return M
