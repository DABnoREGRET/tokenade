require "sinatra"

get "/" do
  "hello"
end

post "/login" do
  json_response({ ok: true })
end
