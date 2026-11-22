<?php
// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The page of stress/server-week's small web shop. The lab PC asks it three
// things, and each answer is one line or a few:
//
//   ?op=read&key=K          item K of 10000, which Valkey keeps for 60
//                           seconds after PostgreSQL gave it: "item K VALUE"
//   ?op=list                the 20 newest writes: "list N", then "SEQ DATA"
//   ?op=write&seq=N&data=D  the write is committed, and then "ok N" says so
//
// PostgreSQL is reached over 127.0.0.1 as the role week, with the password
// that `database add` showed once and the machine's setup kept in
// .week-database beside this page, which nginx never serves and only the
// account php reads. Valkey is spoken to in RESP over a socket, as the PHP
// bundle has no client for it. A failure of either is a 500 whose first
// line says which, so that the lab PC counts it and the log names it.
//
// The time the page took, from its first line to just before it prints, is
// sent as X-Took-Us in microseconds. The lab PC builds the latencies the
// week is judged by from it, as the lab PC may be busy with other
// machines' tests and its own round trip says as much of it as of this one.

declare(strict_types=1);

$began = hrtime(true);

const ITEMS = 10000;
const CACHE_SECONDS = 60;
const LATEST = 20;

function fail(int $status, string $words): never
{
    http_response_code($status);
    header('Content-Type: text/plain');
    echo $words, "\n";
    exit;
}

// Sends one command to Valkey and returns its answer: a string, or null for
// a key that is not there.
function valkey(string ...$words): ?string
{
    $socket = fsockopen('127.0.0.1', 6379, $number, $error, 2.0);
    if ($socket === false) {
        throw new RuntimeException("valkey: $error");
    }
    stream_set_timeout($socket, 5);
    $request = '*' . count($words) . "\r\n";
    foreach ($words as $word) {
        $request .= '$' . strlen($word) . "\r\n" . $word . "\r\n";
    }
    fwrite($socket, $request);
    $line = fgets($socket);
    if ($line === false) {
        throw new RuntimeException('valkey: no answer');
    }
    $kind = $line[0];
    $rest = rtrim(substr($line, 1), "\r\n");
    if ($kind === '+') {
        $answer = $rest;
    } elseif ($kind === '$' && $rest === '-1') {
        $answer = null;
    } elseif ($kind === '$') {
        $answer = stream_get_contents($socket, (int) $rest + 2);
        $answer = $answer === false ? null : substr($answer, 0, (int) $rest);
    } else {
        throw new RuntimeException("valkey: $line");
    }
    fclose($socket);
    return $answer;
}

function database(): PDO
{
    $password = trim((string) file_get_contents(__DIR__ . '/.week-database'));
    return new PDO('pgsql:host=127.0.0.1;dbname=week', 'week', $password, [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
    ]);
}

function read(int $key): string
{
    if ($key < 1 || $key > ITEMS) {
        fail(400, 'key is from 1 to ' . ITEMS);
    }
    $value = valkey('GET', "item:$key");
    if ($value === null) {
        $statement = database()->prepare('SELECT value FROM week_items WHERE id = ?');
        $statement->execute([$key]);
        $value = $statement->fetchColumn();
        if ($value === false) {
            fail(404, "no item $key");
        }
        valkey('SET', "item:$key", $value, 'EX', (string) CACHE_SECONDS);
    }
    return "item $key $value\n";
}

function newest(): string
{
    $rows = database()->query('SELECT seq, data FROM week_writes ORDER BY seq DESC LIMIT ' . LATEST)
        ->fetchAll(PDO::FETCH_NUM);
    $text = 'list ' . count($rows) . "\n";
    foreach ($rows as [$seq, $data]) {
        $text .= "$seq $data\n";
    }
    return $text;
}

function write(int $seq, string $data): string
{
    if ($seq < 1 || preg_match('/^[0-9a-f]{1,64}$/', $data) !== 1) {
        fail(400, 'seq is a positive number and data is up to 64 hexadecimal digits');
    }
    $statement = database()->prepare('INSERT INTO week_writes (seq, data) VALUES (?, ?)');
    $statement->execute([$seq, $data]);
    return "ok $seq\n";
}

try {
    $answer = match ($_GET['op'] ?? '') {
        'read' => read((int) ($_GET['key'] ?? 0)),
        'list' => newest(),
        'write' => write((int) ($_GET['seq'] ?? 0), (string) ($_GET['data'] ?? '')),
        default => fail(400, 'op is read, list or write'),
    };
} catch (Throwable $error) {
    fail(500, 'error: ' . $error->getMessage());
}
header('Content-Type: text/plain');
header('X-Took-Us: ' . intdiv(hrtime(true) - $began, 1000));
echo $answer;
