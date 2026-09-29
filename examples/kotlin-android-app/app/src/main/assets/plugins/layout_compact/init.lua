-- Section order for the demo screen. The host renders in this order and
-- ignores unknown sections, so plugins can never break the layout.
local LayoutCompact = {}
LayoutCompact.__index = LayoutCompact

function LayoutCompact.new(_) return setmetatable({}, LayoutCompact) end

function LayoutCompact:layout() return { order = { "header", "body", "flags", "diagnostics" } } end

return LayoutCompact
