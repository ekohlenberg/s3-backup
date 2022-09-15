using System;
using System.Collections.Generic;

namespace s3b
{
    public class s3bMSSqlTemplate : Dictionary<string, string>
    {
        public s3bMSSqlTemplate()
        {
            Add("newer",
                @"select distinct fldr.* from local_file f
                inner join local_folder fldr on
	                fldr.id = f.folder_id 
                where
	                fldr.backup_set_id=$(id) and
	                (f.current_update > isnull( f.previous_update, '1900-01-01') or
					(fldr.stage + '.' + fldr.status <> 'upload.complete'))
"

                );
        }
    }
}
