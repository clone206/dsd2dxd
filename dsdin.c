/**
* DSD Unpack - https://github.com/michaelburton/dsdunpack
*
* Copyright (c) 2014 by Michael Burton.
*
* This program is free software; you can redistribute it and/or modify
* it under the terms of the GNU General Public License as published by
* the Free Software Foundation; either version 2 of the License, or
* (at your option) any later version.
*
* This program is distributed in the hope that it will be useful,
* but WITHOUT ANY WARRANTY; without even the implied warranty of
* MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
* GNU General Public License for more details.
*
* You should have received a copy of the GNU General Public License
* along with this program; if not, write to the Free Software
* Foundation, Inc., 59 Temple Place, Suite 330, Boston, MA  02111-1307  USA
*
*/

#include <stdlib.h>
#include <stdio.h>
#include <string.h>

#include "dsdin.h"

/* Forward declarations of format-specific read functions */
extern dsd_reader_funcs_t *dsdiff_reader_funcs();
extern dsd_reader_funcs_t *dsf_reader_funcs();

/* Reading */

extern int dsd_reader_open(FILE *fp, dsd_reader_t *reader)
{
    int result = 0;

    /* Find out the format of the input file by reading its 'magic number' */
    if (fread(&reader->container_format, 4, 1, fp) == 1) {
        rewind(fp);

        switch (reader->container_format) {
        case DSD_FORMAT_DSDIFF:
            reader->impl = dsdiff_reader_funcs();
            break;
        case DSD_FORMAT_DSF:
            reader->impl = dsf_reader_funcs();
            break;
        default:         /* Unknown format */
            reader->impl = NULL;
        }

        if (reader->impl && (result = reader->impl->open(fp, reader)) == 1) {
            reader->input = fp;
        }
    }

    return result;
}

extern size_t dsd_reader_read(char *buf, size_t len, dsd_reader_t *reader)
{
    return reader->impl->read(buf, len, reader);
}

extern uint32_t dsd_reader_next_chunk(dsd_reader_t *reader)
{
    return reader->impl->next_chunk(reader);
}

extern void dsd_reader_close(dsd_reader_t *reader)
{
    reader->impl->close(reader);
    if (reader->prvt) {
        free(reader->prvt);
        reader->prvt = NULL;
    }
    if (reader->input) {
        fclose(reader->input);
        reader->input = NULL;
    }
}