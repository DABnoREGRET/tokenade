<?php
use Illuminate\Support\Facades\Route;
use App\Http\Controllers\AuthController;

Route::get('/', fn () => view('welcome'));
Route::post('/login', [AuthController::class, 'login']);
