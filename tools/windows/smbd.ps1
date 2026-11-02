# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Windows' own SMB client against Dukovo's smbd (ADRs 0228, 0229). CI's
# Windows file sharing job runs it on the smbd boot test's image, which
# `ob test boot --case smbd --image-only` writes. It needs PowerShell 7,
# QEMU (Chocolatey's qemu package) and a Windows whose client reaches a
# server on a port other than 445 (`net use /TCPPORT`, Windows 11 24H2
# and Windows Server 2025), as Windows' own server holds 445 here.
#
#   pwsh tools/windows/smbd.ps1 <smbd.img>
#
# QEMU boots the image without KVM, its port 445 forwarded from port
# 4450 of this machine's loopback. /system/tests/posix/smbdtest makes
# alice's account, sets her sharing password and shares her Public
# folder twice, to read and write and only to read, and says it is
# ready. Then Windows' client, signed in as alice over SMB 3:
#
# - maps Public, reads the file Dukovo wrote, writes one and makes a
#   folder, and lists them;
# - shows a file's access list with icacls, adds root to it, and is
#   refused a denying entry;
# - reads Readonly and is refused a write to it;
# - is refused a wrong password;
# - writes done.txt, after which smbdtest checks what Windows did and
#   its sign-ins, as the smbd boot test's host does with Samba's client.

param([Parameter(Mandatory)] [string] $Image)

$ErrorActionPreference = 'Stop'
$Image = (Resolve-Path -LiteralPath $Image).Path
$port = 4450
$password = 'through the looking-glass'
$temp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$work = Join-Path $temp 'smbd'
New-Item -ItemType Directory -Force -Path $work | Out-Null
$serial = Join-Path $work 'serial.log'
Remove-Item -LiteralPath $serial -ErrorAction SilentlyContinue

# QEMU, and its UEFI firmware with a fresh variable store.
$qemuDir = Join-Path $env:ProgramFiles 'qemu'
$qemu = Join-Path $qemuDir 'qemu-system-x86_64.exe'
if (-not (Test-Path -LiteralPath $qemu)) {
    throw "No QEMU at ${qemu}; install it with choco install qemu"
}
& $qemu --version
$code = Join-Path $work 'code.fd'
$vars = Join-Path $work 'vars.fd'
Copy-Item -LiteralPath (Join-Path $qemuDir 'share\edk2-x86_64-code.fd') -Destination $code -Force
Copy-Item -LiteralPath (Join-Path $qemuDir 'share\edk2-i386-vars.fd') -Destination $vars -Force

# The machine the smbd boot test boots, as ob's QEMU arguments make it
# (tools/xtask/src/qemu.rs), the port given when it starts.
$arguments = @(
    '-M', 'q35', '-m', '1G', '-smp', '2', '-accel', 'tcg', '-cpu', 'max',
    '-drive', "if=pflash,format=raw,readonly=on,file=$code",
    '-drive', "if=pflash,format=raw,file=$vars",
    '-drive', "if=none,id=boot,format=raw,snapshot=on,file=$Image",
    '-device', 'qemu-xhci,id=xhci',
    '-device', 'usb-storage,bus=xhci.0,drive=boot,removable=on,bootindex=0',
    '-device', 'isa-debug-exit,iobase=0xf4,iosize=0x04',
    '-no-reboot', '-display', 'none', '-monitor', 'none',
    '-serial', "file:$serial",
    '-netdev', "user,id=n0,hostfwd=tcp:127.0.0.1:$port-:445",
    '-device', 'virtio-net-pci,netdev=n0'
)
$start = [Diagnostics.ProcessStartInfo]::new($qemu)
foreach ($argument in $arguments) {
    $start.ArgumentList.Add($argument)
}
$start.UseShellExecute = $false
$vm = [Diagnostics.Process]::Start($start)

# What the guest has written to its serial port so far.
function Read-Serial {
    if (-not (Test-Path -LiteralPath $serial)) {
        return ''
    }
    $stream = [IO.File]::Open($serial, 'Open', 'Read', 'ReadWrite')
    try {
        return [IO.StreamReader]::new($stream).ReadToEnd()
    } finally {
        $stream.Dispose()
    }
}

# The lines of the serial log that tell why the test failed: smbdtest's
# own, and what the boot tests count as the kernel complaining
# (kernel_complaints in tools/xtask/src/test.rs).
function Get-Failures([string] $Text) {
    return ($Text -split "`n" | Where-Object {
            ($_ -match 'smbdtest: FAILED|with preemption held off|KiB kernel stack left|ACPI: error: |panicked') -or
            ($_.Contains('Lock checker: ') -and -not $_.Contains('Lock checker: on; ') -and
                -not $_.Contains('Lock checker: off'))
        }) -join "`n"
}

# Waits up to $Seconds for each of $Lines in the serial log.
function Wait-Serial([string[]] $Lines, [int] $Seconds) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ($true) {
        $text = Read-Serial
        $failures = Get-Failures $text
        if ($failures) {
            throw "smbdtest failed:`n$failures"
        }
        if (@($Lines | Where-Object { -not $text.Contains($_) }).Count -eq 0) {
            return
        }
        if ($vm.HasExited) {
            throw "QEMU exited ($($vm.ExitCode)) before the guest wrote $($Lines -join ', ')"
        }
        if ((Get-Date) -gt $deadline) {
            throw "The guest did not write $($Lines -join ', ') in $Seconds s"
        }
        Start-Sleep -Seconds 2
    }
}

# Runs net.exe with $Arguments; returns whether it succeeded, and what it
# printed.
function Invoke-Net([string[]] $Arguments) {
    $text = & net.exe @Arguments 2>&1 | Out-String
    return [pscustomobject]@{ Ok = ($LASTEXITCODE -eq 0); Text = $text }
}

# Maps $Share of the guest's smbd as $Drive, signed in as alice with
# $Secret.
function Connect-Share([string] $Drive, [string] $Share, [string] $Secret) {
    return Invoke-Net @('use', $Drive, "\\127.0.0.1\$Share", "/TCPPORT:$port",
        '/USER:alice', $Secret, '/PERSISTENT:NO')
}

# Runs icacls.exe with $Arguments, as Invoke-Net runs net.exe.
function Invoke-Icacls([string[]] $Arguments) {
    $text = & icacls.exe @Arguments 2>&1 | Out-String
    return [pscustomobject]@{ Ok = ($LASTEXITCODE -eq 0); Text = $text }
}

function Assert-That([bool] $Condition, [string] $What) {
    if (-not $Condition) {
        throw "Windows' client did not $What"
    }
    Write-Host "  ok: $What"
}

try {
    # TCG boots the image in minutes.
    Wait-Serial @('smbdtest: ready for the host') 1200

    # Public, once netd has the guest's address: QEMU forwards to the
    # address its DHCP server gives.
    $deadline = (Get-Date).AddSeconds(120)
    do {
        $mapped = Connect-Share 'P:' 'Public' $password
        if (-not $mapped.Ok) {
            Start-Sleep -Seconds 3
        }
    } until ($mapped.Ok -or (Get-Date) -gt $deadline)
    Write-Host $mapped.Text
    Assert-That $mapped.Ok "map Public as alice"
    Write-Host (Get-SmbConnection | Format-List ServerName, ShareName, UserName, Dialect,
            Encrypted, Signed | Out-String)
    $connection = Get-SmbConnection -ServerName 127.0.0.1 -ShareName Public
    Assert-That ($connection.Dialect -like '3.*' -and $connection.Encrypted) `
        "sign in over SMB 3 with encryption"

    # Both ways, and a folder.
    $got = [IO.File]::ReadAllText('P:\from-dukovo.txt')
    Assert-That ($got -ceq "written on Dukovo`n") "read from-dukovo.txt (got `"$got`")"
    [IO.File]::WriteAllText('P:\from-host.txt', "written on the host`n")
    New-Item -ItemType Directory -Path 'P:\made-by-host' | Out-Null
    $names = @(Get-ChildItem -LiteralPath 'P:\' -Force | ForEach-Object Name)
    Write-Host "  Public holds: $($names -join ', ')"
    Assert-That (($names -ccontains 'from-host.txt') -and ($names -ccontains 'made-by-host')) `
        "list what it wrote"

    # The file's access list: alice owns it, and Everyone is the others'.
    # Windows names alice by asking the server over lsarpc, or shows her
    # SID, S-1-22-1-1000, as Samba gives Unix's users.
    $shown = Invoke-Icacls @('P:\from-dukovo.txt')
    Write-Host $shown.Text
    Assert-That ($shown.Ok -and ($shown.Text -match 'Unix User\\alice|S-1-22-1-1000') -and
        ($shown.Text -match 'Everyone')) "show the file's access list"
    $owner = (Get-Acl -LiteralPath 'P:\from-dukovo.txt').GetOwner(
        [Security.Principal.SecurityIdentifier]).Value
    Assert-That ($owner -eq 'S-1-22-1-1000') "show alice as the owner (got $owner)"
    # root, S-1-22-1-0, may read it.
    $granted = Invoke-Icacls @('P:\from-dukovo.txt', '/grant', '*S-1-22-1-0:(R)')
    Write-Host $granted.Text
    Assert-That $granted.Ok "add root to the file's access list"
    $shown = Invoke-Icacls @('P:\from-dukovo.txt')
    Write-Host $shown.Text
    Assert-That ($shown.Text -match '(Unix User\\root|S-1-22-1-0):\(R\)') `
        "show root in the file's access list"
    # A denying entry has no place in a POSIX list.
    $denied = Invoke-Icacls @('P:\from-dukovo.txt', '/deny', '*S-1-22-1-0:(W)')
    Write-Host $denied.Text
    Assert-That (-not $denied.Ok) "be refused a denying entry"

    # Read, but not written, in the read-only share.
    $mapped = Connect-Share 'R:' 'Readonly' $password
    Write-Host $mapped.Text
    Assert-That $mapped.Ok "map Readonly"
    $got = [IO.File]::ReadAllText('R:\from-dukovo.txt')
    Assert-That ($got -ceq "written on Dukovo`n") "read from Readonly"
    $refused = $false
    try {
        [IO.File]::WriteAllText('R:\refused.txt', "never written`n")
    } catch {
        Write-Host "  Readonly refused the write: $($_.Exception.Message)"
        $refused = $true
    }
    Assert-That $refused "be refused a write to Readonly"

    # A wrong password, with no session open to the guest.
    Invoke-Net @('use', 'P:', '/DELETE', '/Y') | Out-Null
    Invoke-Net @('use', 'R:', '/DELETE', '/Y') | Out-Null
    $wrong = Connect-Share 'P:' 'Public' 'not her password'
    Write-Host $wrong.Text
    Assert-That (-not $wrong.Ok) "be refused a wrong password"

    # The last file, which tells the guest Windows is done.
    $mapped = Connect-Share 'P:' 'Public' $password
    Assert-That $mapped.Ok "map Public again"
    [IO.File]::WriteAllText('P:\done.txt', "done`n")
    Invoke-Net @('use', 'P:', '/DELETE', '/Y') | Out-Null

    Wait-Serial @('smbdtest: every check passed',
        '(/system/tests/posix/smbdtest) exited with code 0x0') 600
    Write-Host 'smbd passed against Windows'' client'
} finally {
    if (-not $vm.HasExited) {
        $vm.Kill()
    }
    Write-Host '--- the guest''s serial log, last lines ---'
    Write-Host ((Read-Serial) -split "`n" | Select-Object -Last 60 | Out-String)
}
