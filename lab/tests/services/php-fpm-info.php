<?php
// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The page of services/php-fpm that checks what PHP-FPM gives its pages:
// one word for each check that held, `ok-NAME`, and FAIL-NAME for one that
// did not, then the number the request asked with.
function c($name, $held)
{
    echo ($held ? "ok-" : "FAIL-") . $name . " ";
}

// The FastCGI interface, the account the workers run as, and opcache.
c("sapi", PHP_SAPI == "fpm-fcgi");
c("user", posix_getpwuid(posix_geteuid())["name"] == "php");
c("opcache", opcache_get_status()["opcache_enabled"]);
// A session, and a POST field.
session_start();
c("session", session_id() != "");
c("method", $_SERVER["REQUEST_METHOD"] == "POST");
c("post", ($_POST["a"] ?? "") == "1");
// A file into the folder the administrator gave the account, and none into
// the site's own.
c("upload", @file_put_contents(__DIR__ . "/uploads/u.txt", "x") === 1);
c("readonly", @file_put_contents(__DIR__ . "/n.txt", "x") === false);
echo "n=", $_GET["n"] ?? 0, "\n";
