using System;
using System.Collections.Generic;

namespace s3b
{
    public class s3bSqliteTemplate : System.Collections.Generic.Dictionary<string, string>
    {
        public s3bSqliteTemplate()
        {
            Add("newer",
                @"select distinct fldr.* from local_file f
                            inner join local_folder fldr on
	                            fldr.id = f.folder_id 
                            where
	                            fldr.backup_set_id=$(id) and
	                            (f.current_update > ifnull( f.previous_update, '01/01/1900') or
								(fldr.stage || '.' || fldr.status <> 'clean.complete'))
"

                );
        }
    }
}
