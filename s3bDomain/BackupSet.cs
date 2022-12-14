using System;
using System.Collections.Generic;
using System.Text;
using System.Reflection;
using System.IO;

namespace s3b
{
    public class BackupSet :  Model
    {
        public BackupSet()
        {
            tableName = "backup_set";
        }

        
        public List<LocalFolder> workFolders = new List<LocalFolder>();
        public Dictionary<long, LocalFolder> localFolders = new Dictionary<long, LocalFolder>();
        private Dictionary<string, LocalFolder> _uploadedFolders = new Dictionary<string, LocalFolder>();

        public Dictionary<string, LocalFolder> getUploadedFolders()
        {
            _uploadedFolders.Clear();

            foreach( LocalFolder fldr in localFolders.Values)
            {
                if (!_uploadedFolders.ContainsKey( fldr.encrypted_file_name ))
                {
                    _uploadedFolders.Add(fldr.encrypted_file_name, fldr);
                }
            }

            return _uploadedFolders;
        }

        public static BackupSet factory(Model args)
        {
            Config config = (Config)args;

            BackupSet bset = new BackupSet();

            /*
            Config.getConfig().setValue("temp", Config.getConfig().getString("s3b.temp"));
            Config.getConfig().setValue("bucket", bucket);
            Config.getConfig().setValue("backup_folder", folder);
            Config.getConfig().setValue("root_folder_path", root_folder_path);
            */
            string folder = Config.getConfig().getString("folder");


            bset.root_folder_path = Path.GetFullPath(folder);

            bset.upload_target = config.getString("bucket");

            return bset;
        }
        public long id
        {
            get
            {
                return Convert.ToInt32(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }
        public string root_folder_path
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public string upload_target
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public DateTime last_backup_datetime
        {
            get
            {
                return Convert.ToDateTime(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

    }
}
