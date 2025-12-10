param(
  [Parameter(Mandatory=$true)] [string] $Format,
  [Parameter(Mandatory=$true)] [string] $Endianness,
  [Parameter(Mandatory=$true)] [string] $Level,
  [Parameter(Mandatory=$true)] [string] $InputFile
)

# Build the Rust project
cargo build

# Pipe input file into dsd2dxd and stream to ffplay (stereo float)
Get-Content -AsByteStream -Raw $InputFile |
  .\target\debug\dsd2dxd -f $Format -b 32 -d F -e $Endianness --level=$Level |
  ffplay -f f32le -ar 352.8k -ch_layout stereo -i -
