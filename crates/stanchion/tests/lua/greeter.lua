local Greeter = {}
Greeter.__index = Greeter

function Greeter.new(greeting)
  return setmetatable({ greeting = greeting, count = 0 }, Greeter)
end

function Greeter.describe()
  return "greets people"
end

function Greeter:greet(who)
  self.count = self.count + 1
  return self.greeting .. ", " .. who
end

return Greeter
