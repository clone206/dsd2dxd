# Ensure dsd2dxd is installed to cargo bin
cargo install --path .

# Play back various resample rates from different test inputs
dsd2dxd -r 88200   .\test\1kHz_stereo_p.dsf | ffplay -f s24le -ar 88200  -ch_layout stereo -i -
dsd2dxd -r 176400  .\test\1kHz_stereo_p.dsf | ffplay -f s24le -ar 176400 -ch_layout stereo -i -
dsd2dxd -r 352800  .\test\1kHz_stereo_p.dsf | ffplay -f s24le -ar 352800 -ch_layout stereo -i -
dsd2dxd -r 88200   .\test\1kHz_stereo_128.dsf | ffplay -f s24le -ar 88200  -ch_layout stereo -i -
dsd2dxd -r 176400  .\test\1kHz_stereo_128.dsf | ffplay -f s24le -ar 176400 -ch_layout stereo -i -
dsd2dxd -r 352800  .\test\1kHz_stereo_128.dsf | ffplay -f s24le -ar 352800 -ch_layout stereo -i -

dsd2dxd -r 88200   .\test\1kHz_stereo_256.dsf | ffplay -f s24le -ar 88200  -ch_layout stereo -i -
dsd2dxd -r 176400  .\test\1kHz_stereo_256.dsf | ffplay -f s24le -ar 176400 -ch_layout stereo -i -
dsd2dxd -r 352800  .\test\1kHz_stereo_256.dsf | ffplay -f s24le -ar 352800 -ch_layout stereo -i -

# Run same conversions discarding stdout (PowerShell: redirect to $null)
dsd2dxd -r 88200   .\test\1kHz_stereo_p.dsf    > $null
dsd2dxd -r 176400  .\test\1kHz_stereo_p.dsf    > $null
dsd2dxd -r 352800  .\test\1kHz_stereo_p.dsf    > $null
dsd2dxd -r 88200   .\test\1kHz_stereo_128.dsf  > $null
dsd2dxd -r 176400  .\test\1kHz_stereo_128.dsf  > $null
dsd2dxd -r 352800  .\test\1kHz_stereo_128.dsf  > $null

dsd2dxd -r 88200   .\test\1kHz_stereo_256.dsf  > $null
dsd2dxd -r 176400  .\test\1kHz_stereo_256.dsf  > $null
dsd2dxd -r 352800  .\test\1kHz_stereo_256.dsf  > $null
