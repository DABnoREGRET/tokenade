from django.http import HttpResponse
from .models import User

def home(request):
    return HttpResponse("hi")

def login(request):
    return HttpResponse("login")

def logout(request):
    return HttpResponse("bye")

def user_detail(request, pk):
    u = User.objects.get(pk=pk)
    return HttpResponse(u.name)
