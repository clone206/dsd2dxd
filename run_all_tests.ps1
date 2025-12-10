# Clean output directory
Remove-Item -Recurse -Force .\out\* -ErrorAction SilentlyContinue

# Run test suites (PowerShell equivalents)
pwsh .\test_all_44k_mults.ps1
pwsh .\test_all_48k_mults.ps1

# Final command: replicate stdin redirection with Get-Content -AsByteStream
Get-Content -AsByteStream -Raw .\test\1kHz_stereo_p.dsd |
  dsd2dxd -R -a -o w -f p -e l -r 88200 -p out . -

# Build/play specific tests via PowerShell scripts
pwsh .\build_test_mono.ps1 P 24 L 4 .\test\1kHz_mono_p.dsd
