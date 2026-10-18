SECRET_KEY = "dev"
DEBUG = True
ALLOWED_HOSTS = ["*"]
INSTALLED_APPS = ["django.contrib.admin", "django.contrib.auth", "myapp"]
ROOT_URLCONF = "myapp.urls"
DATABASES = {"default": {"ENGINE": "django.db.backends.sqlite3", "NAME": "db.sqlite3"}}
