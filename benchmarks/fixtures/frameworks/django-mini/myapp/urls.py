from django.urls import path
from . import views

urlpatterns = [
    path("", views.home),
    path("login/", views.login),
    path("logout/", views.logout),
    path("users/<int:pk>/", views.user_detail),
]
