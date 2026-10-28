$ErrorActionPreference = 'Stop'

# Fetch the complete release installer before executing it.
$source = Invoke-RestMethod -Uri 'https://github.com/jolars/metewand/releases/latest/download/metewand-cli-installer.ps1'
$installer = [scriptblock]::Create($source)
& $installer @args
