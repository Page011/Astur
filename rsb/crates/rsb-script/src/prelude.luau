--!nonstrict
-- The rsb module, written in Luau on top of a small set of native functions
-- (N). It runs once per VM before the sandbox is switched on, and returns the
-- entry points the Rust runtime drives (E). Nothing here is reachable by
-- widget code except through the frozen `rsb` table.
--
-- Everything created while a widget runs belongs to a scope; scopes form a
-- tree (instance -> element -> effect runs -> ...). Disposing a scope
-- disposes its children and runs its cleanups, so a re-run never piles up
-- timers, subscriptions or processes.

local N, K = ...

local error, pcall, type, tostring, tonumber = error, pcall, type, tostring, tonumber
local setmetatable, getmetatable, rawget, rawset, rawequal = setmetatable, getmetatable, rawget, rawset, rawequal
local select, next, pairs, ipairs = select, next, pairs, ipairs
local tconcat, tfreeze, tsort = table.concat, table.freeze, table.sort
local yield, isyieldable = coroutine.yield, coroutine.isyieldable
local info = debug.info
local floor = math.floor
local sformat, sgsub = string.format, string.gsub

local CLEAN, CHECK, DIRTY = 0, 1, 2
local EFFECT, COMPUTED = 1, 2
local MAX_RUNS = K.max_runs

local cur_observer = nil
local cur_owner = nil
-- Inside rsb.untrack a read is meant to be a one-time read: no setup warning for it.
local untracking = 0
local no_yield = 0
local INSTANCES = {}
local IS_SIGNAL = setmetatable({}, { __mode = "k" })
local E = {}
local rsb = {}

-- ---------------------------------------------------------------- helpers

local function deep_eq(a, b, depth)
	if rawequal(a, b) then
		return true
	end
	if type(a) ~= "table" or type(b) ~= "table" then
		return false
	end
	depth = depth or 0
	if depth > 8 then
		return false
	end
	for k, v in pairs(a) do
		if not deep_eq(v, b[k], depth + 1) then
			return false
		end
	end
	for k in pairs(b) do
		if a[k] == nil then
			return false
		end
	end
	return true
end

local function shallow_eq(a, b)
	if a == nil or b == nil then
		return a == b
	end
	for k, v in pairs(a) do
		if b[k] ~= v then
			return false
		end
	end
	for k in pairs(b) do
		if a[k] == nil then
			return false
		end
	end
	return true
end

-- Raises "<prefix><name> does not exist; did you mean <prefix><near>?". `level` counts from this
-- function (3 = whoever indexed the table, when called from an __index function).
local function unknown_name(prefix, name, names, level)
	local near = N.suggest(name, names)
	if near then
		error(prefix .. name .. " does not exist; did you mean " .. prefix .. near .. "?", level)
	end
	error(prefix .. name .. " does not exist", level)
end

-- Metatables are shared by every widget of a VM: a widget must not be able to
-- rewrite them (getmetatable(x).__index = ...), so they and their method tables are frozen.
local function freeze_mt(mt)
	if type(mt.__index) == "table" then
		tfreeze(mt.__index)
	end
	return tfreeze(mt)
end

local function strict_table(prefix, tbl, names, special)
	setmetatable(tbl, freeze_mt({
		__index = function(_, k)
			if type(k) ~= "string" then
				return nil
			end
			if special then
				local v = special(k)
				if v ~= nil then
					return v
				end
			end
			unknown_name(prefix, k, names, 3)
		end,
		__newindex = function(_, k)
			error(prefix:sub(1, -2) .. " is read-only", 2)
		end,
	}))
	return tfreeze(tbl)
end

-- ---------------------------------------------------------------- scopes

local function new_scope(parent)
	local s = { state = CLEAN, disposed = false }
	if parent then
		s.parent = parent
		s.inst = parent.inst
		local kids = parent.kids
		if not kids then
			kids = {}
			parent.kids = kids
		end
		kids[s] = true
	end
	return s
end

local dispose_scope

local function reset_scope(s)
	local kids = s.kids
	if kids then
		s.kids = nil
		for k in pairs(kids) do
			dispose_scope(k)
		end
	end
	local c = s.cleanups
	if c then
		s.cleanups = nil
		for i = #c, 1, -1 do
			local ok, e = pcall(c[i])
			if not ok then
				if N.is_killed() then
					error(e, 0)
				end
				N.report_error(s.inst and s.inst.id or 0, tostring(e))
			end
		end
	end
end

dispose_scope = function(s)
	if s.disposed then
		return
	end
	s.disposed = true
	reset_scope(s)
	local srcs = s.sources
	if srcs then
		s.sources = nil
		for i = 1, #srcs do
			srcs[i].observers[s] = nil
		end
	end
	local p = s.parent
	if p then
		local pk = p.kids
		if pk then
			pk[s] = nil
		end
	end
end

local function add_cleanup(scope, fn)
	local c = scope.cleanups
	if not c then
		c = {}
		scope.cleanups = c
	end
	c[#c + 1] = fn
end

-- A function that runs `fn` owned by `scope`, outside any observer.
local function bind_owner(scope, fn)
	return function(...)
		cur_owner = scope
		cur_observer = nil
		no_yield = 0
		return fn(...)
	end
end

-- ---------------------------------------------------------------- signals

local mark

local function enqueue(node)
	local inst = node.inst
	if not inst then
		return
	end
	local q = inst.queue
	q[#q + 1] = node
	if not inst.pending then
		inst.pending = true
		N.pending(inst.id)
	end
end

mark = function(node, state)
	if node.state < state then
		local was_clean = node.state == CLEAN
		node.state = state
		if was_clean then
			if node.kind == EFFECT then
				enqueue(node)
			end
			local obs = node.observers
			if obs then
				for o in pairs(obs) do
					mark(o, CHECK)
				end
			end
		end
	end
end

local function track(src)
	local o = cur_observer
	if o then
		if not src.observers[o] then
			src.observers[o] = true
			local srcs = o.sources
			if not srcs then
				srcs = {}
				o.sources = srcs
			end
			srcs[#srcs + 1] = src
		end
	else
		local owner = cur_owner
		if owner then
			local inst = owner.inst
			if inst and inst.in_setup and owner == inst.scope and not inst.warned and untracking == 0 then
				inst.warned = true
				N.warn_untracked(inst.id)
			end
		end
	end
end

local function sig_set(s, v)
	if rawequal(rawget(s, "v"), v) then
		return
	end
	rawset(s, "v", v)
	for o in pairs(s.observers) do
		mark(o, DIRTY)
	end
end

local function sig_set_eq(s, v)
	if type(v) == "table" and deep_eq(rawget(s, "v"), v) then
		return
	end
	sig_set(s, v)
end

local update_if_necessary

local function run_node(node)
	local inst = node.inst
	-- Only a re-run counts toward the update-loop cap: building a big tree
	-- runs every binding once and is not a loop.
	if inst and node.ran then
		local n = inst.runs + 1
		inst.runs = n
		if n > MAX_RUNS then
			N.update_loop(inst.id)
		end
	end
	node.ran = true
	node.state = CLEAN
	reset_scope(node)
	local srcs = node.sources
	if srcs then
		-- Emptied in place: the next run fills the same table again.
		for i = 1, #srcs do
			srcs[i].observers[node] = nil
			srcs[i] = nil
		end
	end
	local po, pb, pn = cur_owner, cur_observer, no_yield
	cur_owner, cur_observer, no_yield = node, node, pn + 1
	local ok, res = pcall(node.fn)
	cur_owner, cur_observer, no_yield = po, pb, pn
	if not ok then
		if N.is_killed() then
			error(res, 0)
		end
		N.report_error(inst and inst.id or 0, tostring(res))
		return
	end
	if node.kind == COMPUTED then
		if not rawequal(rawget(node, "v"), res) then
			rawset(node, "v", res)
			for o in pairs(node.observers) do
				o.state = DIRTY
			end
		end
	elseif type(res) == "function" then
		add_cleanup(node, res)
	end
end

update_if_necessary = function(node)
	if node.state == CHECK then
		local srcs = node.sources
		if srcs then
			for i = 1, #srcs do
				local s = srcs[i]
				if s.fn then
					update_if_necessary(s)
				end
				if node.state == DIRTY then
					break
				end
			end
		end
	end
	if node.state == DIRTY then
		run_node(node)
	else
		node.state = CLEAN
	end
end

local SigMT = {}
SigMT.__index = function(t, k)
	if k == "value" then
		track(t)
		return rawget(t, "v")
	elseif k == "peek" then
		return function(self)
			return rawget(self, "v")
		end
	elseif k == "update" then
		return function(self, fn)
			sig_set(self, fn(rawget(self, "v")))
		end
	end
	return nil
end
SigMT.__newindex = function(t, k, v)
	if k == "value" then
		sig_set(t, v)
	else
		error("a signal only has .value", 2)
	end
end

local function new_signal(v)
	local s = { v = v, observers = {} }
	IS_SIGNAL[s] = true
	return setmetatable(s, SigMT)
end

local CompMT = {}
CompMT.__index = function(t, k)
	if k == "value" then
		update_if_necessary(t)
		track(t)
		return rawget(t, "v")
	elseif k == "peek" then
		return function(self)
			update_if_necessary(self)
			return rawget(self, "v")
		end
	end
	return nil
end
-- The engine keeps its own bookkeeping fields on the node; only `.value` is
-- off limits.
CompMT.__newindex = function(t, k, v)
	if k == "value" then
		error("a computed value is read-only", 2)
	end
	rawset(t, k, v)
end

local function create_node(kind, fn, owner)
	local n = {
		kind = kind,
		fn = fn,
		state = DIRTY,
		parent = owner,
		inst = owner and owner.inst,
		disposed = false,
	}
	if kind == COMPUTED then
		n.observers = {}
	end
	if owner then
		local kids = owner.kids
		if not kids then
			kids = {}
			owner.kids = kids
		end
		kids[n] = true
	end
	return n
end

local function create_effect(owner, fn)
	local n = create_node(EFFECT, fn, owner)
	update_if_necessary(n)
	return n
end

local function is_dynamic(v)
	local t = type(v)
	return t == "function" or (t == "table" and IS_SIGNAL[v] == true) or (t == "table" and getmetatable(v) == CompMT)
end

-- Turns a Reactive<T> into a function.
local function as_function(v)
	if type(v) == "function" then
		return v
	end
	return function()
		return v.value
	end
end

function rsb.state(v)
	return new_signal(v)
end

function rsb.computed(fn)
	if type(fn) ~= "function" then
		error("rsb.computed needs a function", 2)
	end
	local n = create_node(COMPUTED, fn, cur_owner)
	return setmetatable(n, CompMT)
end

function rsb.effect(fn)
	if type(fn) ~= "function" then
		error("rsb.effect needs a function", 2)
	end
	local owner = cur_owner
	if not owner or not owner.inst then
		error("rsb.effect must be called while a widget is setting up or running", 2)
	end
	create_effect(owner, fn)
end

function rsb.batch(fn)
	-- Effects run after the current call, so every write in fn is one update.
	fn()
end

function rsb.untrack(fn)
	local pb = cur_observer
	cur_observer = nil
	untracking += 1
	local ok, r = pcall(fn)
	untracking -= 1
	cur_observer = pb
	if not ok then
		error(r, 0)
	end
	return r
end

function rsb.on_cleanup(fn)
	local owner = cur_owner
	if not owner then
		error("rsb.on_cleanup must be called while a widget is setting up or running", 2)
	end
	if type(fn) ~= "function" then
		error("rsb.on_cleanup needs a function", 2)
	end
	add_cleanup(owner, fn)
end

-- ---------------------------------------------------------------- elements

local function dirty(node)
	local inst = node.inst
	if inst and not inst.dirty then
		inst.dirty = true
		N.dirty(inst.id)
	end
end

local BASE_PROPS = {
	id = true,
	class = true,
	style = true,
	key = true,
	visible = true,
	on_click = true,
	on_right_click = true,
	on_middle_click = true,
	on_wheel = true,
	on_enter = true,
	on_leave = true,
}
local KIND_PROPS = {
	box = {},
	text = { text = true },
	icon = { name = true, glyph = true },
	image = { src = true, width = true, height = true },
}
local KIND_PROP_LISTS = K.prop_lists
local HANDLER_KEYS = {
	on_click = "click",
	on_right_click = "right_click",
	on_middle_click = "middle_click",
	on_wheel = "wheel",
	on_enter = "enter",
	on_leave = "leave",
}
local UNITLESS = K.unitless

local function conv_class(v)
	local t = type(v)
	if t == "string" then
		return v
	elseif t == "table" then
		local parts = {}
		for i = 1, #v do
			local s = v[i]
			if type(s) ~= "string" then
				return nil, "class entries must be strings"
			end
			parts[i] = s
		end
		return tconcat(parts, " ")
	elseif v == nil or v == false then
		return ""
	end
	return nil, "class must be a string or a list of strings, got " .. type(v)
end

local function conv_style(v)
	if v == nil then
		return {}
	end
	if type(v) ~= "table" then
		return nil, "style must be a table, got " .. type(v)
	end
	local out = {}
	for k, val in pairs(v) do
		if type(k) ~= "string" then
			return nil, "style keys must be strings"
		end
		local css = sgsub(k, "_", "-")
		local tv = type(val)
		if tv == "string" then
			out[css] = val
		elseif tv == "number" then
			if UNITLESS[css] then
				out[css] = tostring(val)
			else
				out[css] = tostring(val) .. "px"
			end
		elseif val ~= nil then
			return nil, "style " .. k .. " must be a string or a number, got " .. tv
		end
	end
	return out
end

local function conv_visible(v)
	return v ~= false
end

local function conv_text(v)
	local t = type(v)
	if t == "string" then
		return v
	elseif t == "number" then
		return tostring(v)
	elseif v == nil then
		return ""
	end
	return nil, "text must be a string or a number, got " .. t
end

local function conv_string(what)
	return function(v)
		if type(v) == "string" then
			return v
		elseif v == nil then
			return ""
		end
		return nil, what .. " must be a string, got " .. type(v)
	end
end

local PROP_CONV = {
	class = conv_class,
	style = conv_style,
	visible = conv_visible,
	text = conv_text,
	name = conv_string("name"),
	glyph = conv_string("glyph"),
	src = conv_string("src"),
}

local function prop_error(kind, key)
	if K.reserved_props[key] then
		return key .. " is not available in this version"
	end
	local near = N.suggest(key, KIND_PROP_LISTS[kind])
	local msg = "unknown prop '" .. key .. "' for rsb." .. kind
	if near then
		msg = msg .. "; did you mean '" .. near .. "'?"
	elseif K.style_names[key] then
		msg = msg .. "; style properties go in style = { " .. key .. " = ... }"
	end
	return msg
end

local make_element

local function child_def(c, level)
	local t = type(c)
	if t == "string" or t == "number" then
		return N.unwrap(make_element("text", { text = c }, "row", 1))
	elseif t == "userdata" then
		local d = N.unwrap(c)
		if d then
			return d
		end
	end
	error(
		"a child must be an element, a string, a number or a function returning a list; got " .. t
			.. (t == "table" and " (make elements with rsb.text, rsb.box, ...)" or ""),
		level
	)
end

-- `level` counts from the caller of make_element up to widget code.
make_element = function(kind, props, direction, level)
	level = level or 3
	if type(props) ~= "table" then
		if kind == "text" then
			props = { text = props }
		elseif kind == "icon" then
			props = { name = props }
		else
			error("rsb." .. (direction == "column" and "column" or "box") .. " takes a table of props", level)
		end
	end
	local allowed = KIND_PROPS[kind]
	local def = { kind = kind, direction = direction, children = {} }
	local maxn = 0
	local has_style = false
	for k, v in pairs(props) do
		local tk = type(k)
		if tk == "string" then
			if allowed[k] or BASE_PROPS[k] then
				local handler = HANDLER_KEYS[k]
				if handler then
					if type(v) ~= "function" then
						error(k .. " must be a function", level)
					end
					local h = def.handlers
					if not h then
						h = {}
						def.handlers = h
					end
					h[handler] = v
				elseif k == "key" or k == "id" then
					if type(v) ~= "string" and type(v) ~= "number" then
						error(k .. " must be a string", level)
					end
					def[k] = tostring(v)
				elseif k == "width" or k == "height" then
					if type(v) ~= "number" then
						error(k .. " must be a number", level)
					end
					def[k] = v
				else
					local conv = PROP_CONV[k]
					if is_dynamic(v) then
						def[k] = { fn = as_function(v) }
					else
						local val, e = conv(v)
						if e then
							error(e, level)
						end
						def[k] = { val = val }
					end
					if k == "style" then
						has_style = true
					end
				end
			else
				error(prop_error(kind, k), level)
			end
		elseif tk == "number" then
			if k > maxn then
				maxn = k
			end
		else
			error("prop names must be strings", level)
		end
	end
	if maxn > 0 then
		if kind ~= "box" then
			error("rsb." .. kind .. " cannot have children", level)
		end
		local children = def.children
		for i = 1, maxn do
			local c = props[i]
			if c ~= nil then
				if type(c) == "function" then
					children[#children + 1] = { fn = c }
				else
					children[#children + 1] = { def = child_def(c, level) }
				end
			end
		end
	end
	if kind == "icon" and def.name == nil and def.glyph == nil then
		error("rsb.icon needs a name or a glyph", level)
	elseif kind == "image" and def.src == nil then
		error("rsb.image needs a src", level)
	end
	if has_style then
		local src, line = info(level, "sl")
		if src then
			def.file = sgsub(src, "^=", "")
			def.line = line
		end
	end
	return N.wrap(def)
end

function rsb.box(props)
	return make_element("box", props, "row", 3)
end
function rsb.row(props)
	return make_element("box", props, "row", 3)
end
function rsb.column(props)
	return make_element("box", props, "column", 3)
end
function rsb.text(props)
	return make_element("text", props, "row", 3)
end
function rsb.icon(props)
	return make_element("icon", props, "row", 3)
end
function rsb.image(props)
	return make_element("image", props, "row", 3)
end

local mount

local function reconcile(slot, list, parent)
	if type(list) ~= "table" then
		error("a children function must return a list of elements, got " .. type(list), 0)
	end
	local old = slot.nodes
	local new_nodes, used = {}, {}
	local maxn = 0
	for k in pairs(list) do
		if type(k) == "number" and k > maxn then
			maxn = k
		end
	end
	local n = 0
	for i = 1, maxn do
		local c = list[i]
		if c ~= nil then
			local def = child_def(c, 0)
			local live = def.live
			local node
			if live and live.slot == slot and not live.scope.disposed then
				if used[live] then
					error("the same element appears twice in one list", 0)
				end
				node = live
			else
				node = mount(def, parent.scope, parent.inst)
				node.slot = slot
			end
			used[node] = true
			n = n + 1
			new_nodes[n] = node
		end
	end
	for i = 1, #old do
		local o = old[i]
		if not used[o] then
			dispose_scope(o.scope)
			o.def.live = nil
		end
	end
	slot.nodes = new_nodes
end

mount = function(def, parent_scope, inst)
	local live = def.live
	if live and not live.scope.disposed then
		error("an element can only be placed once", 0)
	end
	local scope = new_scope(parent_scope)
	local node = {
		kind = def.kind,
		scope = scope,
		inst = inst,
		key = def.key,
		id = def.id,
		file = def.file,
		line = def.line,
		dir = def.direction,
		def = def,
		w = def.width,
		h = def.height,
		vis = true,
	}
	def.live = node
	-- Props bind under the names the snapshot reads.
	local function bind_as(field, key, eq)
		local p = def[key]
		if p == nil then
			return
		end
		if p.fn == nil then
			node[field] = p.val
			return
		end
		local conv = PROP_CONV[key]
		create_effect(scope, function()
			local v, e = conv(p.fn())
			if e then
				error(key .. ": " .. e, 0)
			end
			local old = node[field]
			local same
			if eq then
				same = eq(old, v)
			else
				same = old == v
			end
			if not same then
				node[field] = v
				dirty(node)
			end
		end)
	end
	bind_as("cls", "class")
	bind_as("style", "style", shallow_eq)
	bind_as("vis", "visible")
	bind_as("text", "text")
	bind_as("iname", "name")
	bind_as("iglyph", "glyph")
	bind_as("src", "src")
	local handlers = def.handlers
	if handlers then
		for name, fn in pairs(handlers) do
			local hid = N.handler(bind_owner(scope, fn))
			node["h_" .. name] = hid
			add_cleanup(scope, function()
				N.unhandler(hid)
			end)
		end
	end
	local slots = {}
	node.slots = slots
	local children = def.children
	for i = 1, #children do
		local c = children[i]
		if c.fn then
			local slot = { dyn = true, nodes = {} }
			slots[i] = slot
			local fn = c.fn
			create_effect(scope, function()
				local list = fn()
				reconcile(slot, list, node)
				dirty(node)
			end)
		else
			slots[i] = mount(c.def, scope, inst)
		end
	end
	return node
end

-- rsb.each(list, render, key)
function rsb.each(list, render, keyfn)
	if type(render) ~= "function" then
		error("rsb.each needs a render function as its second argument", 2)
	end
	if keyfn ~= nil and type(keyfn) ~= "function" then
		error("rsb.each: the third argument must be a function returning a key", 2)
	end
	local owner = cur_owner
	local cache = {}
	-- A render function that takes the index is rebuilt when the item moves;
	-- one that ignores it keeps its element (and state) wherever the item goes.
	local uses_index = (info(render, "a")) >= 2
	return function()
		local items = list
		if type(list) == "function" then
			items = list()
		elseif type(list) == "table" and IS_SIGNAL[list] then
			items = list.value
		elseif type(list) == "table" and getmetatable(list) == CompMT then
			items = list.value
		end
		if items == nil then
			items = {}
		end
		if type(items) ~= "table" then
			error("rsb.each needs a list, got " .. type(items), 0)
		end
		local out, seen = {}, {}
		for i = 1, #items do
			local item = items[i]
			local k
			if keyfn then
				k = tostring(keyfn(item))
			else
				k = i
			end
			if seen[k] then
				k = tostring(k) .. "#" .. i
			end
			local c = cache[k]
			if c and (c.index == i or not uses_index) and not c.scope.disposed and deep_eq(c.item, item) then
				-- unchanged: keep the element and everything it owns
			else
				if c then
					dispose_scope(c.scope)
				end
				local scope = new_scope(owner)
				local po, pb = cur_owner, cur_observer
				cur_owner, cur_observer = scope, nil
				local ok, r = pcall(render, item, i)
				cur_owner, cur_observer = po, pb
				if not ok then
					dispose_scope(scope)
					error(r, 0)
				end
				local def = N.unwrap(r)
				if not def then
					dispose_scope(scope)
					error("the render function of rsb.each must return an element", 0)
				end
				if def.key == nil then
					def.key = "each:" .. tostring(k)
				end
				c = { item = item, index = i, def = def, elem = r, scope = scope }
				cache[k] = c
			end
			seen[k] = true
			out[#out + 1] = c.elem
		end
		for k, c in pairs(cache) do
			if not seen[k] then
				dispose_scope(c.scope)
				cache[k] = nil
			end
		end
		return out
	end
end

-- ---------------------------------------------------------------- tasks and waiting

local function check_yield(name)
	if no_yield > 0 or not isyieldable() then
		error(name .. " waits for a result; call it inside rsb.task.spawn, rsb.poll, a timer or a handler", 3)
	end
end

-- Yields the running task and returns what the runtime resumes it with.
local function wait_result()
	local owner = cur_owner
	local a, b = yield()
	cur_owner, cur_observer, no_yield = owner, nil, 0
	return a, b
end

local CancelMT = {
	__index = {
		cancel = function(self)
			local f = rawget(self, "cancel_fn")
			if f then
				f(rawget(self, "id"))
			end
		end,
	},
}

local function handle(id, cancel_fn)
	return setmetatable({ id = id, cancel_fn = cancel_fn }, CancelMT)
end

local task = {}
function task.spawn(fn, ...)
	if type(fn) ~= "function" then
		error("rsb.task.spawn needs a function", 2)
	end
	local owner = cur_owner
	local po, pb, pn = cur_owner, cur_observer, no_yield
	local body = bind_owner(owner, fn)
	local id = N.spawn(function(...)
		no_yield = 0
		return body(...)
	end, ...)
	cur_owner, cur_observer, no_yield = po, pb, pn
	return handle(id, N.cancel)
end
function task.sleep(d)
	check_yield("rsb.task.sleep")
	N.sleep(d)
	wait_result()
end
function task.cancel(t)
	if type(t) ~= "table" or rawget(t, "id") == nil then
		error("rsb.task.cancel needs a task", 2)
	end
	N.cancel(rawget(t, "id"))
end
rsb.task = strict_table("rsb.task.", task, K.names.task)

local timer = {}
local function timer_fn(kind)
	return function(d, fn)
		if type(fn) ~= "function" then
			error("rsb.timer." .. kind .. " needs a function", 2)
		end
		local owner = cur_owner
		local id = N.timer(kind, d, bind_owner(owner, fn))
		if owner then
			add_cleanup(owner, function()
				N.timer_cancel(id)
			end)
		end
		return handle(id, N.timer_cancel)
	end
end
timer.every = timer_fn("every")
timer.after = timer_fn("after")
rsb.timer = strict_table("rsb.timer.", timer, K.names.timer)

local VALID_PRECISION = { second = true, minute = true, hour = true }

function rsb.clock(precision, timezone)
	precision = precision or "minute"
	if not VALID_PRECISION[precision] then
		error('rsb.clock precision must be "second", "minute" or "hour"', 2)
	end
	local s = new_signal(N.time_now(timezone))
	local owner = cur_owner
	local id = N.clock_timer(precision, timezone, function()
		sig_set(s, N.time_now(timezone))
	end)
	if owner then
		add_cleanup(owner, function()
			N.timer_cancel(id)
		end)
	end
	return s
end

local time = {}
function time.now(timezone)
	return N.time_now(timezone)
end
function time.format(t, fmt)
	return N.time_format(t, fmt)
end
function time.precision(fmt)
	return N.time_precision(fmt)
end
rsb.time = strict_table("rsb.time.", time, K.names.time)

local PollMT = {
	__index = function(t, k)
		if k == "value" then
			local s = rawget(t, "vsig")
			track(s)
			return rawget(s, "v")
		elseif k == "error" then
			local s = rawget(t, "esig")
			track(s)
			return rawget(s, "v")
		elseif k == "peek" then
			return function(self)
				return rawget(rawget(self, "vsig"), "v")
			end
		end
		return nil
	end,
	__newindex = function()
		error("a poll result is read-only", 2)
	end,
}

function rsb.poll(every, fn)
	if type(fn) ~= "function" then
		error("rsb.poll needs a function as its second argument", 2)
	end
	local vsig, esig = new_signal(nil), new_signal(nil)
	local owner = cur_owner
	local function run()
		local ok, r = pcall(fn)
		if ok then
			sig_set(vsig, r)
			sig_set(esig, nil)
		else
			if N.is_killed() then
				error(r, 0)
			end
			local msg = tostring(r)
			N.log("warn", "poll failed: " .. msg)
			sig_set(esig, msg)
		end
	end
	local bound = bind_owner(owner, run)
	local id = N.timer("every", every, bound)
	if owner then
		add_cleanup(owner, function()
			N.timer_cancel(id)
		end)
	end
	-- First run now, as a task.
	local po, pb, pn = cur_owner, cur_observer, no_yield
	N.spawn(function()
		no_yield = 0
		return bound()
	end)
	cur_owner, cur_observer, no_yield = po, pb, pn
	return setmetatable({ vsig = vsig, esig = esig }, PollMT)
end

-- ---------------------------------------------------------------- services

local svc_state = {}
local services = {}
for name, schema in pairs(K.svc) do
	local st = {
		raw = {},
		sigs = {},
		status = new_signal("off"),
		stale = new_signal(false),
		error = new_signal(nil),
	}
	svc_state[name] = st
	local cmds = {}
	for cmd in pairs(schema.commands) do
		cmds[cmd] = function(_, ...)
			N.svc_command(name, cmd, ...)
		end
	end
	local interval_fn
	if schema.sampled then
		interval_fn = function(_, d)
			N.svc_interval(name, d)
		end
	end
	local function read(sig)
		track(sig)
		return rawget(sig, "v")
	end
	local prefix = "rsb.services." .. name .. "."
	services[name] = setmetatable({}, freeze_mt({
		__index = function(_, k)
			if type(k) ~= "string" then
				return nil
			end
			-- Commands and :interval do their own subscribing (or none).
			local c = cmds[k]
			if c then
				return c
			end
			if k == "interval" and interval_fn then
				return interval_fn
			end
			local known = k == "status" or k == "stale" or k == "error" or schema.fields[k]
			if not known then
				unknown_name(prefix, k, schema.names, 3)
			end
			N.touch(name)
			if k == "status" then
				return read(st.status)
			elseif k == "stale" then
				return read(st.stale)
			elseif k == "error" then
				return read(st.error)
			end
			local s = st.sigs[k]
			if not s then
				s = new_signal(st.raw[k])
				st.sigs[k] = s
			end
			return read(s)
		end,
		__newindex = function()
			error("rsb.services." .. name .. " is read-only", 2)
		end,
	}))
	tfreeze(services[name])
end
rsb.services = strict_table("rsb.services.", services, K.names.services)

function E.svc_event(name, ev)
	local st = svc_state[name]
	if not st then
		return
	end
	sig_set(st.status, ev.status)
	sig_set(st.stale, ev.stale)
	sig_set(st.error, ev.error)
	local value = ev.value
	if value ~= nil then
		st.raw = value
		for k, s in pairs(st.sigs) do
			sig_set_eq(s, value[k])
		end
	end
end

local theme_sig = new_signal(K.dark)
rsb.theme = strict_table("rsb.theme.", {}, K.names.theme, function(k)
	if k == "dark" then
		track(theme_sig)
		return rawget(theme_sig, "v") == true
	end
	return nil
end)
function E.set_theme(dark)
	sig_set(theme_sig, dark)
end

-- ---------------------------------------------------------------- I/O

local function finish_response(r)
	return setmetatable(r, {
		__index = {
			json = function(self)
				return N.json_decode(self.body)
			end,
		},
	})
end

local exec = {}
function exec.run(command, options)
	check_yield("rsb.exec.run")
	N.exec_run(command, options)
	return (wait_result())
end
function exec.shell(line, options)
	check_yield("rsb.exec.shell")
	N.exec_shell(line, options)
	return (wait_result())
end
function exec.spawn(command, options)
	local owner = cur_owner
	options = options or {}
	local on_line, on_exit = options.on_line, options.on_exit
	if on_line ~= nil and type(on_line) ~= "function" then
		error("on_line must be a function", 2)
	end
	if on_exit ~= nil and type(on_exit) ~= "function" then
		error("on_exit must be a function", 2)
	end
	local id = N.exec_spawn(command, options, on_line and bind_owner(owner, on_line), on_exit and bind_owner(owner, on_exit))
	if owner then
		add_cleanup(owner, function()
			N.io_cancel(id)
		end)
	end
	return setmetatable({ id = id }, {
		__index = {
			kill = function(self)
				N.io_cancel(rawget(self, "id"))
			end,
		},
	})
end
function exec.lines(command)
	local s = new_signal(nil)
	exec.spawn(command, {
		on_line = function(line)
			sig_set(s, line)
		end,
	})
	return s
end
rsb.exec = strict_table("rsb.exec.", exec, K.names.exec)

local file = {}
function file.read(path, options)
	check_yield("rsb.file.read")
	N.file_read(path, options)
	return wait_result()
end
function file.watch(path, options)
	local vsig, esig = new_signal(nil), new_signal(nil)
	local owner = cur_owner
	local id = N.file_watch(path, options, bind_owner(owner, function(text, err)
		sig_set(vsig, text)
		sig_set(esig, err)
	end))
	if owner then
		add_cleanup(owner, function()
			N.io_cancel(id)
		end)
	end
	return setmetatable({ vsig = vsig, esig = esig }, PollMT)
end
rsb.file = strict_table("rsb.file.", file, K.names.file)

local http = {}
function http.get(url, options)
	check_yield("rsb.http.get")
	N.http("GET", url, nil, options)
	return finish_response((wait_result()))
end
function http.post(url, body, options)
	check_yield("rsb.http.post")
	N.http("POST", url, body, options)
	return finish_response((wait_result()))
end
rsb.http = strict_table("rsb.http.", http, K.names.http)

rsb.json = strict_table("rsb.json.", {
	decode = function(text)
		return N.json_decode(text)
	end,
	encode = function(v)
		return N.json_encode(v)
	end,
}, K.names.json)

function rsb.open_url(url)
	N.open_url(url)
end

-- ---------------------------------------------------------------- events, log, format

rsb.events = strict_table("rsb.events.", {
	on = function(name, fn)
		if type(name) ~= "string" then
			error("rsb.events.on needs an event name", 2)
		end
		if type(fn) ~= "function" then
			error("rsb.events.on needs a function", 2)
		end
		local owner = cur_owner
		local id = N.events_on(name, bind_owner(owner, fn))
		if owner then
			add_cleanup(owner, function()
				N.events_off(id)
			end)
		end
		return handle(id, N.events_off)
	end,
	emit = function(name, data)
		if type(name) ~= "string" then
			error("rsb.events.emit needs an event name", 2)
		end
		N.events_emit(name, data)
	end,
}, K.names.events)

local function log_fn(level)
	return function(...)
		local n = select("#", ...)
		local parts = {}
		for i = 1, n do
			parts[i] = tostring((select(i, ...)))
		end
		N.log(level, tconcat(parts, " "))
	end
end
rsb.log = strict_table("rsb.log.", { info = log_fn("info"), warn = log_fn("warn"), error = log_fn("error") }, K.names.log)

local UNITS = { "B", "KB", "MB", "GB", "TB" }
local function format_bytes(n)
	if type(n) ~= "number" then
		error("rsb.format.bytes needs a number", 2)
	end
	local i = 1
	local v = n
	while v >= 1024 and i < #UNITS do
		v = v / 1024
		i = i + 1
	end
	if i == 1 then
		return sformat("%d B", floor(v + 0.5))
	end
	return sformat("%.1f %s", v, UNITS[i])
end
rsb.format = strict_table("rsb.format.", {
	bytes = format_bytes,
	rate = function(n)
		if type(n) ~= "number" then
			error("rsb.format.rate needs a number", 2)
		end
		return format_bytes(n) .. "/s"
	end,
	percent = function(n)
		if type(n) ~= "number" then
			error("rsb.format.percent needs a number", 2)
		end
		return sformat("%d%%", floor(n + 0.5))
	end,
}, K.names.format)

-- ---------------------------------------------------------------- widget

local WIDGETS = setmetatable({}, { __mode = "k" })
local WIDGET_KEYS = { "name", "options", "optional", "permissions", "setup" }
local WIDGET_KEY_SET = { name = true, options = true, optional = true, permissions = true, setup = true }

function rsb.widget(spec)
	if type(spec) ~= "table" then
		error("rsb.widget needs a table: rsb.widget { setup = function(props, ctx) ... end }", 2)
	end
	for k in pairs(spec) do
		if not WIDGET_KEY_SET[k] then
			local near = type(k) == "string" and N.suggest(k, WIDGET_KEYS)
			error("unknown field '" .. tostring(k) .. "' in rsb.widget" .. (near and ("; did you mean '" .. near .. "'?") or ""), 2)
		end
	end
	if type(spec.setup) ~= "function" then
		error("rsb.widget needs a setup function", 2)
	end
	if spec.name ~= nil and type(spec.name) ~= "string" then
		error("rsb.widget: name must be a string", 2)
	end
	if spec.options ~= nil and type(spec.options) ~= "table" then
		error("rsb.widget: options must be a table", 2)
	end
	local h = tfreeze({ name = spec.name or "" })
	WIDGETS[h] = spec
	return h
end

rsb.version = 1

-- Names for did-you-mean on the module itself.
setmetatable(rsb, freeze_mt({
	__index = function(_, k)
		if type(k) ~= "string" then
			return nil
		end
		if K.reserved[k] then
			error("rsb." .. k .. " is not available in this version", 2)
		end
		local hint = K.hints[k]
		if hint then
			error("rsb." .. k .. " does not exist; use rsb." .. hint, 2)
		end
		unknown_name("rsb.", k, K.names.rsb, 3)
	end,
	__newindex = function()
		error("rsb is read-only", 2)
	end,
}))
tfreeze(rsb)
freeze_mt(SigMT)
freeze_mt(CompMT)
freeze_mt(PollMT)
freeze_mt(CancelMT)

-- ---------------------------------------------------------------- globals

-- Replaced or removed before the sandbox is switched on.
print = log_fn("info")
loadstring = nil
getfenv = nil
setfenv = nil

local raw_collectgarbage = collectgarbage
collectgarbage = function(opt)
	if opt ~= "count" then
		error('collectgarbage only supports "count"', 2)
	end
	return raw_collectgarbage("count")
end

local raw_sort = table.sort
local MAX_SORT = K.max_sort
table.sort = function(t, cmp)
	if cmp == nil and type(t) == "table" and #t > MAX_SORT then
		error("table.sort without a comparison function is limited to " .. MAX_SORT .. " items", 2)
	end
	return raw_sort(t, cmp)
end

require = function(spec)
	if spec == "@rsb" then
		return rsb
	end
	if type(spec) ~= "string" then
		error("require needs a module path", 2)
	end
	return N.require(spec, (info(2, "s")))
end

-- ---------------------------------------------------------------- entry points

function E.reset()
	cur_observer, cur_owner, no_yield = nil, nil, 0
end

-- Runs a loaded chunk (a widget file or a module) where nothing may yield.
function E.load(fn)
	no_yield = no_yield + 1
	local ok, r = pcall(fn)
	no_yield = no_yield - 1
	if not ok then
		error(r, 0)
	end
	return r
end

function E.widget_spec(h)
	local s = WIDGETS[h]
	if not s then
		return nil
	end
	return s.name or "", s.options, s.optional, s.permissions
end

function E.start(id, h, props, ctx)
	local spec = WIDGETS[h]
	local inst = { id = id, queue = {}, pending = false, dirty = false, turn = -1, runs = 0, in_setup = true, ctx = ctx }
	INSTANCES[id] = inst
	local scope = { state = CLEAN, disposed = false, inst = inst }
	inst.scope = scope
	local po, pb, pn = cur_owner, cur_observer, no_yield
	cur_owner, cur_observer, no_yield = scope, nil, 1
	local ok, res = pcall(spec.setup, props, ctx)
	cur_owner, cur_observer, no_yield = po, pb, pn
	inst.in_setup = false
	if not ok then
		error(res, 0)
	end
	if res ~= nil then
		local def = N.unwrap(res)
		if not def then
			error("setup must return an element made by rsb.box, rsb.row, rsb.text, ... or nil", 0)
		end
		cur_owner, cur_observer, no_yield = scope, nil, 1
		local ok2, node = pcall(mount, def, scope, inst)
		cur_owner, cur_observer, no_yield = po, pb, pn
		if not ok2 then
			error(node, 0)
		end
		inst.root = node
	end
	inst.dirty = true
	N.dirty(id)
end

function E.root(id)
	local inst = INSTANCES[id]
	if not inst then
		return nil
	end
	inst.dirty = false
	return inst.root
end

function E.flush(id, turn)
	local inst = INSTANCES[id]
	if not inst then
		return
	end
	if inst.turn ~= turn then
		inst.turn = turn
		inst.runs = 0
	end
	local q = inst.queue
	local head = 1
	while head <= #q do
		local node = q[head]
		q[head] = false
		head = head + 1
		if node and not node.disposed and node.state ~= CLEAN then
			N.slice()
			update_if_necessary(node)
		end
	end
	inst.queue = {}
	inst.pending = false
end

function E.dispose(id)
	local inst = INSTANCES[id]
	if not inst then
		return
	end
	INSTANCES[id] = nil
	inst.root = nil
	dispose_scope(inst.scope)
end

function E.update_monitor(id, scale)
	local inst = INSTANCES[id]
	if inst and inst.ctx and inst.ctx.monitor then
		inst.ctx.monitor.scale = scale
	end
end

return E, rsb
