defmodule MyAppWeb.Router do
  use MyAppWeb, :router
  scope "/", MyAppWeb do
    get "/", PageController, :home
    get "/login", AuthController, :login
  end
end
